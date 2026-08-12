//! Shared typed command surface for the standalone adapter and Cori's linked
//! workflow dispatch.
//!
//! Keeping parsing and execution in the library prevents the broker and the
//! optional diagnostic binary from drifting into different SAP operations.

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{
    AdapterError, MachineProfile, Result, SapClient, access_token_from_env, load_machine_profile,
    validate_limit, validate_purchase_order_id,
};

/// Maximum serialized result accepted at the broker/Temporal boundary.
pub const MAX_SERIALIZED_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "cori-sap",
    version,
    about = "Read-only SAP S/4HANA Purchase Order adapter"
)]
pub struct AdapterCli {
    /// Select a profile already present in machine-owned sap.toml.
    /// Forbidden when launched as a Cori workflow capability.
    #[arg(long, global = true, value_name = "NAME")]
    profile: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read purchase-order headers and items.
    PurchaseOrders {
        #[command(subcommand)]
        command: PurchaseOrdersCommand,
    },
}

#[derive(Debug, Subcommand)]
enum PurchaseOrdersCommand {
    /// Get one purchase-order header by its SAP key.
    Get {
        #[arg(long, value_name = "PURCHASE_ORDER")]
        id: String,
    },
    /// List purchase-order headers, newest key first.
    List {
        #[arg(long, default_value_t = 50, value_name = "COUNT")]
        limit: u16,
    },
    /// List the items belonging to one purchase order.
    Items {
        #[arg(long, value_name = "PURCHASE_ORDER")]
        id: String,
        #[arg(long, default_value_t = 100, value_name = "COUNT")]
        limit: u16,
    },
}

/// Execute an invocation of the optional standalone diagnostic binary.
///
/// Standalone calls load machine configuration themselves and may source the
/// token from their own process environment. Cori workflow execution uses
/// [`execute_workflow_argv`] instead and never exposes the token to a child.
pub fn execute_standalone(cli: AdapterCli) -> Result<String> {
    let profile = load_machine_profile(cli.profile.as_deref())?;
    let token = access_token_from_env()?;
    execute(profile, token, cli.command)
}

/// Parse and execute the exact read-only `cori-sap` command surface inside the
/// broker process.
///
/// `profile` and `bearer_token` are an indivisible owner/target-bound
/// credential resolved by the broker. The argv cannot select a different
/// profile, URL, HTTP method, header, filter, or request body.
pub fn execute_workflow_argv(
    profile: MachineProfile,
    bearer_token: String,
    argv: &[String],
) -> Result<String> {
    let cli = parse_workflow_argv(argv)?;
    validate_command(&cli.command)?;
    execute(profile, bearer_token, cli.command)
}

/// Parse and validate a workflow command without loading credentials or
/// contacting SAP. Dry-run uses this to reject unsupported or malformed SAP
/// argv with the same contract as real linked dispatch.
pub fn validate_workflow_argv(argv: &[String]) -> Result<()> {
    let cli = parse_workflow_argv(argv)?;
    validate_command(&cli.command)
}

fn parse_workflow_argv(argv: &[String]) -> Result<AdapterCli> {
    let cli = AdapterCli::try_parse_from(argv.iter().map(String::as_str)).map_err(|_| {
        AdapterError::InvalidInput {
            field: "arguments",
            reason: "command line does not match the supported read-only interface",
        }
    })?;
    enforce_workflow_boundary(&cli, true)?;
    Ok(cli)
}

fn validate_command(command: &Command) -> Result<()> {
    match command {
        Command::PurchaseOrders { command } => match command {
            PurchaseOrdersCommand::Get { id } => {
                validate_purchase_order_id(id)?;
            }
            PurchaseOrdersCommand::List { limit } => {
                validate_limit(*limit)?;
            }
            PurchaseOrdersCommand::Items { id, limit } => {
                validate_purchase_order_id(id)?;
                validate_limit(*limit)?;
            }
        },
    }
    Ok(())
}

fn execute(profile: MachineProfile, bearer_token: String, command: Command) -> Result<String> {
    let client = SapClient::new(profile, bearer_token)?;
    match command {
        Command::PurchaseOrders { command } => match command {
            PurchaseOrdersCommand::Get { id } => {
                let id = validate_purchase_order_id(&id)?;
                serialize_json_output(&client.get_purchase_order(id)?)
            }
            PurchaseOrdersCommand::List { limit } => {
                let limit = validate_limit(limit)?;
                serialize_json_output(&client.list_purchase_orders(limit)?)
            }
            PurchaseOrdersCommand::Items { id, limit } => {
                let id = validate_purchase_order_id(&id)?;
                let limit = validate_limit(limit)?;
                serialize_json_output(&client.list_purchase_order_items(id, limit)?)
            }
        },
    }
}

fn enforce_workflow_boundary(cli: &AdapterCli, is_workflow: bool) -> Result<()> {
    if !is_workflow {
        return Ok(());
    }
    if cli.profile.is_some() {
        return Err(AdapterError::WorkflowProfileDenied);
    }
    // Keep this match deliberately exhaustive. Adding any future top-level or
    // purchase-order command must make an explicit workflow-safety decision.
    match &cli.command {
        Command::PurchaseOrders {
            command:
                PurchaseOrdersCommand::Get { .. }
                | PurchaseOrdersCommand::List { .. }
                | PurchaseOrdersCommand::Items { .. },
        } => Ok(()),
    }
}

fn serialize_json_output<T: Serialize>(value: &T) -> Result<String> {
    let output = serde_json::to_string(value).map_err(|_| AdapterError::OutputSerialization)?;
    if output.len() > MAX_SERIALIZED_OUTPUT_BYTES {
        return Err(AdapterError::OutputTooLarge {
            max_bytes: MAX_SERIALIZED_OUTPUT_BYTES,
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::{TcpListener, TcpStream};

    fn cli(command: Command) -> AdapterCli {
        AdapterCli {
            profile: None,
            command,
        }
    }

    #[test]
    fn workflow_boundary_allows_only_typed_reads() {
        for command in [
            PurchaseOrdersCommand::Get {
                id: "4500001234".to_string(),
            },
            PurchaseOrdersCommand::List { limit: 10 },
            PurchaseOrdersCommand::Items {
                id: "4500001234".to_string(),
                limit: 10,
            },
        ] {
            let cli = cli(Command::PurchaseOrders { command });
            enforce_workflow_boundary(&cli, true).expect("typed read is workflow-safe");
        }
    }

    #[test]
    fn workflow_argv_rejects_explicit_profiles_and_unknown_commands() {
        let explicit_profile =
            ["cori-sap", "--profile", "other", "purchase-orders", "list"].map(str::to_string);
        assert_eq!(
            validate_workflow_argv(&explicit_profile),
            Err(AdapterError::WorkflowProfileDenied)
        );

        let unknown = ["cori-sap", "config", "set"].map(str::to_string);
        assert!(matches!(
            validate_workflow_argv(&unknown),
            Err(AdapterError::InvalidInput {
                field: "arguments",
                ..
            })
        ));

        let invalid_limit =
            ["cori-sap", "purchase-orders", "list", "--limit", "0"].map(str::to_string);
        assert!(matches!(
            validate_workflow_argv(&invalid_limit),
            Err(AdapterError::InvalidInput { field: "limit", .. })
        ));
    }

    #[test]
    fn serialized_output_is_bounded_before_returning_to_the_broker() {
        assert_eq!(
            serialize_json_output(&serde_json::json!({"ok": true})),
            Ok(r#"{"ok":true}"#.to_string())
        );
        let oversized = "x".repeat(MAX_SERIALIZED_OUTPUT_BYTES);
        assert_eq!(
            serialize_json_output(&serde_json::json!({"value": oversized})),
            Err(AdapterError::OutputTooLarge {
                max_bytes: MAX_SERIALIZED_OUTPUT_BYTES
            })
        );
    }

    #[test]
    fn linked_workflow_command_executes_against_sap() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock SAP listener");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("mock SAP address")
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("mock SAP accept");
            let request = read_request(&stream);
            let body = r#"{"value":[{"PurchaseOrder":"4500001234","Supplier":"1000"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("mock SAP response");
            request
        });

        let profile = MachineProfile::for_loopback_test(&origin).expect("loopback profile");
        let argv = ["cori-sap", "purchase-orders", "get", "--id", "4500001234"].map(str::to_string);
        let output = execute_workflow_argv(profile, "owner-token".to_string(), &argv)
            .expect("linked workflow output");
        let value: serde_json::Value = serde_json::from_str(&output).expect("output JSON");
        assert_eq!(
            value.pointer("/purchase_order/supplier"),
            Some(&serde_json::json!("1000"))
        );

        let request = server.join().expect("mock SAP server");
        assert!(request.starts_with("get "));
        assert!(request.contains("authorization: bearer owner-token"));
    }

    fn read_request(stream: &TcpStream) -> String {
        let mut reader = BufReader::new(stream.try_clone().expect("clone mock stream"));
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("mock request line");
            if line == "\r\n" || line.is_empty() {
                break;
            }
            request.push_str(&line.to_ascii_lowercase());
        }
        request
    }
}
