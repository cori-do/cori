//! Cross-platform policy for Cori-owned background child processes.
//!
//! The Console is a Windows GUI application. Without an explicit creation
//! flag, Windows gives a console subsystem child a new visible console even
//! when Cori has piped or discarded all of its standard streams. Those
//! short-lived windows are especially noticeable for Deno expression
//! evaluation and capability probes.

/// Reserved SAP credential name used only by the optional standalone adapter.
/// Workflow data-plane reads receive their owner-bound token in broker memory.
pub use cori_sap::ACCESS_TOKEN_ENV as SAP_ACCESS_TOKEN_ENV;

const AMBIENT_SAP_TOKEN_ERROR: &str = "SAP_ACCESS_TOKEN must not be present in the Cori process environment; unset it and use `cori login cori-sap`";

/// Fail closed before a Cori CLI or Console process starts workers or child
/// processes. Removing an inherited variable is insufficient on platforms
/// where the process's initial environment remains readable by the same UID.
pub fn reject_ambient_sap_token() -> Result<(), &'static str> {
    if std::env::var_os(SAP_ACCESS_TOKEN_ENV).is_some() {
        Err(AMBIENT_SAP_TOKEN_ERROR)
    } else {
        Ok(())
    }
}

/// Remove ambient or caller-declared SAP credential material from a child.
/// No broker-spawned child is allowed to inherit it.
pub fn scrub_sap_env(cmd: &mut std::process::Command) {
    cmd.env_remove(SAP_ACCESS_TOKEN_ENV);
}

/// Prevent a non-interactive child process from creating a visible console
/// window on Windows.
///
/// Call this only for children whose input and output Cori owns. Commands
/// deliberately launched for a terminal user must retain their normal
/// console behaviour.
pub fn hide_console_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        // `CREATE_NO_WINDOW` from WinBase.h. Keeping the literal avoids
        // making all workspace targets depend on a Windows bindings crate.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn scrub_reserves_the_standalone_sap_token() {
        let mut command = std::process::Command::new("child");
        command.env(SAP_ACCESS_TOKEN_ENV, "ambient-token");
        scrub_sap_env(&mut command);

        let env: HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new(SAP_ACCESS_TOKEN_ENV)),
            Some(&None)
        );
    }
}
