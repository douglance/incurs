//! A terminal application surface for incurs command graphs.
//!
//! The same command graph that serves a CLI, an MCP server, and a desktop
//! window also runs as a full-screen terminal application. Commands are listed
//! on the left, the selected command's inputs are collected from its schema,
//! and every call goes through `ToolCatalog` — so validation, middleware,
//! config defaults, streaming, and cancellation behave as they do everywhere
//! else.
//!
//! ```no_run
//! use incurs_app_ratatui::TerminalApp;
//!
//! # fn example(cli: &incurs::cli::Cli) -> Result<(), Box<dyn std::error::Error>> {
//! TerminalApp::from_cli(cli)?.title("Todo").run()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Start it from synchronous code
//!
//! [`TerminalApp::run`] builds a [`ToolRunner`], which owns a Tokio runtime,
//! and Tokio panics when a runtime is dropped inside another one. Call this
//! from a `main` that is **not** `#[tokio::main]`.

#![deny(missing_docs)]

pub mod app;
pub mod editor;
pub mod input;
pub mod theme;
pub mod view;

use std::sync::Arc;
use std::time::Duration;

use incurs::cli::Cli;
use incurs::tool::{ToolCatalog, ToolCatalogError};
use incurs_app_model::{AppSession, CallEnvironment, SkillPublisher, ToolRunner};
use ratatui::crossterm::event::{self, Event};

pub use app::{App, Focus};
pub use editor::Editor;
pub use incurs_app_model::{CallEnvironment as Environment, SkillScope};

/// How long a draw loop waits for a key before checking for call updates.
///
/// Short enough that a streaming command's progress appears promptly, long
/// enough that an idle application is not busy-waiting.
const TICK: Duration = Duration::from_millis(16);

/// Builder for the terminal application.
pub struct TerminalApp {
    catalog: ToolCatalog,
    title: Option<String>,
    environment: CallEnvironment,
    skills: Option<SkillPublisher>,
}

impl TerminalApp {
    /// Builds a terminal application over one tool catalog.
    pub fn new(catalog: ToolCatalog) -> Self {
        Self {
            catalog,
            title: None,
            environment: CallEnvironment::default(),
            skills: None,
        }
    }

    /// Builds a terminal application from one CLI, offering its agent skills.
    ///
    /// # Errors
    ///
    /// Returns the collision when two commands would share an exposed name.
    pub fn from_cli(cli: &Cli) -> Result<Self, ToolCatalogError> {
        let publisher = SkillPublisher::from_cli(cli);
        Ok(Self {
            skills: (!publisher.is_empty()).then_some(publisher),
            ..Self::new(cli.try_tool_catalog()?)
        })
    }

    /// Sets the title shown in the frame.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets how calls reach the environment and configuration.
    pub fn environment(mut self, environment: CallEnvironment) -> Self {
        self.environment = environment;
        self
    }

    /// Sets the agent skill publisher the application offers.
    pub fn skills(mut self, skills: Option<SkillPublisher>) -> Self {
        self.skills = skills;
        self
    }

    /// Runs the application until the person quits.
    ///
    /// Takes over the terminal and restores it on the way out, including after
    /// a panic.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::ErrorKind::Unsupported`] when stdout is not a
    /// terminal, and any terminal or runtime failure otherwise.
    pub fn run(self) -> std::io::Result<()> {
        // Without a terminal there is nothing to draw on and no key will ever
        // arrive, so the loop would block forever. Refusing is the honest
        // answer, and it is what makes piping this a clear error rather than a
        // hang.
        if !incurs::pager::stdout_is_interactive() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "the terminal interface needs a terminal; run this without redirecting output",
            ));
        }
        let title = self
            .title
            .clone()
            .unwrap_or_else(|| self.catalog.name().to_string());
        let runner = Arc::new(ToolRunner::new(self.catalog, self.environment)?);

        let mut app = App::new(AppSession::new(runner), title);
        app.set_skills(self.skills);

        let mut terminal = ratatui::init();
        let result = drive(&mut terminal, &mut app);
        ratatui::restore();
        result
    }
}

/// Draws and handles input until the application asks to quit.
fn drive(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| view::draw(frame, app))?;

        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
        {
            input::handle(app, key);
        }

        // Updates arrive on the call runtime, so they are drained rather than
        // awaited; the draw loop never blocks on a command.
        app.drain();
        app.poll_skills();

        if app.quit {
            return Ok(());
        }
    }
}
