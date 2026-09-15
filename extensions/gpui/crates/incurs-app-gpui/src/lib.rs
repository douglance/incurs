//! Native desktop application surface for incurs command graphs.
//!
//! This crate is a Tool Binding. It exposes the Tool Contracts of one
//! [`ToolCatalog`] to a person through a GPUI window, so a command graph that
//! already serves a CLI, HTTP, and MCP can also ship as a double-clickable
//! application for someone who will never open a terminal.
//!
//! Nothing about a command changes. The window lists each MCP-visible leaf
//! command, builds its input form from the contract's JSON Schema, and invokes
//! it through [`ToolCatalog::call`], so validation, middleware, declared
//! environment fields, config defaults, streaming, and cancellation behave
//! exactly as they do on every other surface.
//!
//! # Shipping an existing CLI as an application
//!
//! ```no_run
//! use incurs::cli::Cli;
//! use incurs_app_gpui::DesktopApp;
//!
//! # fn build_cli() -> Cli { Cli::create("todo") }
//! fn main() -> std::io::Result<()> {
//!     let cli = build_cli();
//!     DesktopApp::from_cli(&cli)
//!         .expect("command names must be unique")
//!         .title("Todo")
//!         .run()
//! }
//! ```
//!
//! Use [`crate::bundle`] to wrap the resulting executable in a macOS
//! application bundle a non-technical person can install by dragging it.

#![deny(missing_docs)]

pub mod bundle;
pub mod text_field;
pub mod theme;

#[doc(inline)]
pub use incurs_app_model::{form, rows as render, runtime, session, skills};
mod workbench;

use std::sync::Arc;

use gpui::{
    AppContext, Application, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions,
    px, size,
};
use incurs::cli::Cli;
use incurs::tool::{ToolCatalog, ToolCatalogError};

pub use incurs_app_model::CallEnvironment;
pub use incurs_app_model::{SkillPublisher, SkillReport, SkillScope};
pub use workbench::Workbench;

/// Builder for the desktop application window.
pub struct DesktopApp {
    catalog: ToolCatalog,
    title: Option<SharedString>,
    environment: CallEnvironment,
    skills: Option<SkillPublisher>,
    install_skills_on_launch: bool,
    width: f32,
    height: f32,
}

impl DesktopApp {
    /// Creates an application from a resolved tool catalog.
    pub fn new(catalog: ToolCatalog) -> Self {
        Self {
            catalog,
            title: None,
            environment: CallEnvironment::default(),
            skills: None,
            install_skills_on_launch: false,
            width: 1080.0,
            height: 720.0,
        }
    }

    /// Creates an application from a CLI's command graph.
    ///
    /// # Errors
    ///
    /// Returns [`ToolCatalogError`] when two commands resolve to the same
    /// exposed name, which would otherwise hide one of them from the window.
    pub fn from_cli(cli: &Cli) -> Result<Self, ToolCatalogError> {
        let mut app = Self::new(cli.try_tool_catalog()?);
        app.skills = Some(SkillPublisher::from_cli(cli));
        Ok(app)
    }

    /// Sets the skill publisher offered in the window.
    ///
    /// [`DesktopApp::from_cli`] supplies one already. Set it explicitly to
    /// change its scope or target directory, or pass `None` to hide skill
    /// installation entirely.
    pub fn skills(mut self, skills: Option<SkillPublisher>) -> Self {
        self.skills = skills;
        self
    }

    /// Installs agent skills once when the window opens.
    ///
    /// Off by default. Installing writes into the person's agent
    /// configuration directories, so a shipped application should normally
    /// leave it to the button in the window rather than acting unasked.
    pub fn install_skills_on_launch(mut self, install: bool) -> Self {
        self.install_skills_on_launch = install;
        self
    }

    /// Sets the window and titlebar title.
    ///
    /// Defaults to the CLI name when unset.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets how calls resolve environment values and config defaults.
    pub fn environment(mut self, environment: CallEnvironment) -> Self {
        self.environment = environment;
        self
    }

    /// Sets the initial window size in logical pixels.
    pub fn window_size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Opens the window and runs the application until it is closed.
    ///
    /// This takes over the calling thread, as every desktop platform requires
    /// its event loop to own the main thread.
    ///
    /// # Errors
    ///
    /// Returns the Tokio error when the call runtime cannot be created.
    pub fn run(self) -> std::io::Result<()> {
        let title = self
            .title
            .unwrap_or_else(|| SharedString::from(self.catalog.name().to_string()));
        let runner = Arc::new(runtime::ToolRunner::new(self.catalog, self.environment)?);
        let (width, height) = (self.width, self.height);
        let skills = self.skills;
        let install_on_launch = self.install_skills_on_launch;

        Application::new().run(move |cx| {
            text_field::bind_keys(cx);

            let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.clone()),
                    ..TitlebarOptions::default()
                }),
                window_min_size: Some(size(px(720.0), px(480.0))),
                app_id: Some(title.to_string()),
                ..WindowOptions::default()
            };

            let window = cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    let mut workbench = Workbench::new(runner.clone(), title.clone(), window, cx);
                    workbench.set_skills(skills.clone(), cx);
                    if install_on_launch {
                        workbench.install_skills(cx);
                    }
                    workbench
                })
            });

            if let Ok(window) = window {
                let _ = window.update(cx, |_, window, _| window.activate_window());
            }
            cx.activate(true);
        });

        Ok(())
    }
}
