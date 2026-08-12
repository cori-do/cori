use std::io::Write as _;

use clap::{Parser, error::ErrorKind};

use cori_sap::AdapterError;
use cori_sap::command::{AdapterCli, execute_standalone};

fn main() {
    let cli = match AdapterCli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return;
        }
        Err(_) => {
            let error = AdapterError::InvalidInput {
                field: "arguments",
                reason: "command line does not match the supported read-only interface",
            };
            error.write_json_stderr();
            std::process::exit(error.exit_code());
        }
    };

    match execute_standalone(cli) {
        Ok(output) => {
            let mut stdout = std::io::stdout().lock();
            if stdout
                .write_all(output.as_bytes())
                .and_then(|_| stdout.write_all(b"\n"))
                .is_err()
            {
                let error = AdapterError::OutputSerialization;
                error.write_json_stderr();
                std::process::exit(error.exit_code());
            }
        }
        Err(error) => {
            error.write_json_stderr();
            std::process::exit(error.exit_code());
        }
    }
}
