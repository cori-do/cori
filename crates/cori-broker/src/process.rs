//! Cross-platform policy for Cori-owned background child processes.
//!
//! The Console is a Windows GUI application. Without an explicit creation
//! flag, Windows gives a console subsystem child a new visible console even
//! when Cori has piped or discarded all of its standard streams. Those
//! short-lived windows are especially noticeable for Deno expression
//! evaluation and capability probes.

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
