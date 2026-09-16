//! Colors.
//!
//! Every color is a terminal-palette color rather than a fixed RGB value, so
//! the surface inherits whatever scheme the person already chose for their
//! terminal instead of fighting it.

use ratatui::style::{Color, Style};

/// The styles the terminal surface draws with.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Ordinary content.
    pub text: Style,
    /// Secondary content: labels, hints, help.
    pub muted: Style,
    /// Selection and focus.
    pub accent: Style,
    /// Failures and destructive commands.
    pub danger: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            text: Style::default(),
            muted: Style::default().fg(Color::DarkGray),
            accent: Style::default().fg(Color::Cyan),
            danger: Style::default().fg(Color::Red),
        }
    }
}
