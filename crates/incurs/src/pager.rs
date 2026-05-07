//! Pager helpers for human-facing CLI output.

use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

/// Returns `true` when stdout is interactive and paging makes sense.
pub fn stdout_is_interactive() -> bool {
    std::io::stdout().is_terminal()
}

/// Attempts to write `output` to the configured pager.
///
/// Respects `$PAGER` when set, otherwise falls back to `less -FRX`.
/// Returns `Ok(true)` when a pager was successfully started, `Ok(false)`
/// when no pager could be launched, and `Err(_)` for write failures.
pub fn page_output(output: &str) -> io::Result<bool> {
    let pager = std::env::var("PAGER")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let mut command = if let Some(pager) = pager {
        let mut command = Command::new("sh");
        command.arg("-c").arg(pager);
        command
    } else {
        let mut command = Command::new("less");
        command.args(["-FRX"]);
        command
    };

    command.stdin(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Ok(false),
    };

    if let Some(mut stdin) = child.stdin.take()
        && let Err(error) = stdin.write_all(output.as_bytes())
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        return Err(error);
    }

    let status = child.wait()?;
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_interactive_is_callable() {
        let _ = stdout_is_interactive();
    }

    #[test]
    fn pager_falls_back_when_command_is_missing() {
        let original = std::env::var_os("PAGER");
        unsafe {
            std::env::set_var("PAGER", "__definitely_missing_pager__");
        }

        let result = page_output("hello from incur pager");

        match original {
            Some(value) => unsafe {
                std::env::set_var("PAGER", value);
            },
            None => unsafe {
                std::env::remove_var("PAGER");
            },
        }

        assert!(matches!(result, Ok(false)));
    }
}
