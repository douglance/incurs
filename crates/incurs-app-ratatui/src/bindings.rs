//! Hooks for building your own terminal interface over an incurs command graph.
//!
//! [`crate::TerminalApp`] is a reference application, not the only one. A
//! terminal interface owns its own draw loop, so most of what it needs is
//! already in [`incurs_app_model`], which has no user-interface dependency.
//! What is left is small and easy to get wrong, and it is what this module is.
//!
//! Bring your own widgets and keep your own loop:
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use incurs_app_model::{AppSession, CallEnvironment, RunHandle, ToolRunner};
//! # use incurs_app_ratatui::bindings;
//! # fn example(catalog: incurs::tool::ToolCatalog) -> std::io::Result<()> {
//! bindings::require_terminal()?;
//!
//! let runner = Arc::new(ToolRunner::new(catalog, CallEnvironment::Host)?);
//! let mut session = AppSession::new(runner);
//! let mut running: Option<RunHandle> = None;
//!
//! let mut terminal = ratatui::init();
//! loop {
//!     terminal.draw(|frame| { /* your widgets */ })?;
//!     // your key handling
//!     bindings::drain(&mut session, &mut running);
//! #   break;
//! }
//! ratatui::restore();
//! # Ok(())
//! # }
//! ```
//!
//! Three things make an interface an incurs interface, and none of them is a
//! widget: calls go through the tool catalog, values are coerced by
//! [`incurs_app_model::FormState::arguments`] rather than by the view, and
//! field problems come back from the runtime rather than being invented.

use std::io::ErrorKind;

use incurs_app_model::{AppSession, RunHandle, RunUpdate};

/// Refuses to continue when stdout is not a terminal.
///
/// Without one there is nothing to draw on and no key will ever arrive, so a
/// draw loop spins forever. Call this before taking over the terminal, so a
/// caller that redirects output gets an error rather than a hang.
///
/// # Errors
///
/// Returns [`ErrorKind::Unsupported`] when stdout is not a terminal.
pub fn require_terminal() -> std::io::Result<()> {
    if incurs::pager::stdout_is_interactive() {
        return Ok(());
    }
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "the terminal interface needs a terminal; run this without redirecting output",
    ))
}

/// Applies every update that has arrived, without blocking.
///
/// Calls run on a separate runtime, so a draw loop collects their updates
/// rather than awaiting them. `running` is cleared once the terminal update
/// arrives, which is the signal that the call is over.
///
/// Returns whether anything was applied, so a loop can skip redrawing a frame
/// that would be identical.
pub fn drain(session: &mut AppSession, running: &mut Option<RunHandle>) -> bool {
    let mut applied = false;
    let mut finished = false;

    if let Some(handle) = running.as_mut() {
        while let Ok(update) = handle.updates.try_recv() {
            finished |= matches!(update, RunUpdate::Finished(_));
            session.receive(update);
            applied = true;
        }
    }
    if finished {
        *running = None;
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A captured stdout is not a terminal, which is what the guard is for.
    #[test]
    fn a_captured_stdout_is_refused() {
        let error = require_terminal().expect_err("the test harness captures stdout");

        assert_eq!(error.kind(), ErrorKind::Unsupported);
    }
}
