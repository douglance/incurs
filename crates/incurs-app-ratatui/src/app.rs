//! The terminal application's state.
//!
//! This owns what the view needs and the session does not: which pane has
//! focus, one editor per text field, the search box, and the handle of the call
//! in flight. Everything else is read from the [`AppSession`].

use std::collections::BTreeMap;

use incurs_app_model::form::FieldKind;
use incurs_app_model::{AppSession, RunHandle, RunState, SkillPublisher, SkillReport};

use crate::editor::Editor;

/// Which pane the keyboard is driving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The command list.
    Commands,
    /// The selected command's form.
    Form,
}

/// The terminal application's state.
pub struct App {
    /// Everything that is not specific to a terminal.
    pub session: AppSession,
    /// The application's title.
    pub title: String,
    /// Which pane the keyboard drives.
    pub focus: Focus,
    /// The focused field, as an index into the selected command's fields.
    pub field: usize,
    /// One editor per text-like field. The editors own their text, not the
    /// session; their contents are written into form state before a call.
    pub editors: BTreeMap<String, Editor>,
    /// The command search box.
    pub search: Editor,
    /// Whether the search box is being typed into.
    pub searching: bool,
    /// The selected row of the visible command list.
    pub row: usize,
    /// The call in flight, if any.
    pub handle: Option<RunHandle>,
    /// Whether the help overlay is open.
    pub help: bool,
    /// Whether the raw result is shown instead of labelled rows.
    pub raw: bool,
    /// A transient message shown in the status bar.
    pub status: Option<String>,
    /// Whether the application should exit.
    pub quit: bool,
    /// A skill installation running on the call runtime.
    skill_receiver: Option<std::sync::mpsc::Receiver<Result<SkillReport, String>>>,
}

impl App {
    /// Builds the application over one session.
    pub fn new(session: AppSession, title: impl Into<String>) -> Self {
        let mut app = Self {
            session,
            title: title.into(),
            focus: Focus::Commands,
            field: 0,
            editors: BTreeMap::new(),
            search: Editor::default(),
            searching: false,
            row: 0,
            handle: None,
            help: false,
            raw: false,
            status: None,
            quit: false,
            skill_receiver: None,
        };
        app.rebuild_editors();
        app
    }

    /// Rebuilds one editor per text-like field of the selected command.
    pub fn rebuild_editors(&mut self) {
        self.editors.clear();
        self.field = 0;
        self.raw = false;
        for (name, _placeholder) in self.session.text_fields() {
            let seed = self
                .session
                .state()
                .value(&name)
                .map(|value| value.text.clone())
                .unwrap_or_default();
            self.editors.insert(name, Editor::new(seed));
        }
    }

    /// Returns the indices of commands matching the search box.
    pub fn visible(&self) -> Vec<usize> {
        self.session.visible(self.search.text())
    }

    /// Selects the command shown on one visible row.
    pub fn select_row(&mut self, row: usize) {
        let visible = self.visible();
        if visible.is_empty() {
            return;
        }
        let row = row.min(visible.len() - 1);
        self.row = row;
        self.session.select(visible[row]);
        self.rebuild_editors();
    }

    /// Returns the name and kind of the focused field.
    pub fn focused_field(&self) -> Option<(String, FieldKind)> {
        let model = self.session.form()?;
        let field = model.fields.get(self.field)?;
        Some((field.name.clone(), field.kind.clone()))
    }

    /// Returns how many fields the selected command has.
    pub fn field_count(&self) -> usize {
        self.session
            .form()
            .map(|model| model.fields.len())
            .unwrap_or(0)
    }

    /// Copies every editor back into the form state.
    ///
    /// The editors are the source of truth for text, so this must run before
    /// arguments are collected. Without it, typing reaches no command.
    pub fn sync_editors(&mut self) {
        let texts: Vec<(String, String)> = self
            .editors
            .iter()
            .map(|(name, editor)| (name.clone(), editor.text().to_string()))
            .collect();
        for (name, text) in texts {
            self.session.state_mut().value_mut(&name).text = text;
        }
    }

    /// Starts the selected command.
    pub fn run(&mut self) {
        if self.session.is_running() {
            return;
        }
        self.sync_editors();
        match self.session.start_run() {
            Ok(handle) => {
                self.raw = false;
                self.status = None;
                self.handle = Some(handle);
            }
            Err(issues) => {
                self.status = issues
                    .first()
                    .map(|issue| format!("{}: {}", issue.field, issue.message));
            }
        }
    }

    /// Requests cancellation of the call in flight.
    pub fn cancel(&mut self) {
        self.session.cancel();
    }

    /// Drains every update that has arrived without blocking.
    ///
    /// Returns whether anything changed, so the caller can avoid redrawing a
    /// frame that would be identical.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        let mut finished = false;
        if let Some(handle) = self.handle.as_mut() {
            while let Ok(update) = handle.updates.try_recv() {
                finished |= matches!(update, incurs_app_model::RunUpdate::Finished(_));
                self.session.receive(update);
                changed = true;
            }
        }
        if finished {
            self.handle = None;
        }
        changed
    }

    /// Installs agent skills, reporting the outcome in the status bar.
    ///
    /// Runs on the call runtime rather than blocking the draw loop.
    pub fn install_skills(&mut self) {
        let Some(publisher) = self.session.begin_skill_install() else {
            return;
        };
        self.status = Some("Installing skills…".to_string());
        let (sender, receiver) = std::sync::mpsc::channel();
        self.session.runner().spawn(async move {
            let _ = sender.send(match publisher.install().await {
                Ok(result) => Ok(SkillReport::from_result(&result)),
                Err(error) => Err(error.to_string()),
            });
        });
        self.skill_receiver = Some(receiver);
    }

    /// Collects a finished skill installation, if one has completed.
    pub fn poll_skills(&mut self) -> bool {
        let Some(receiver) = self.skill_receiver.as_ref() else {
            return false;
        };
        match receiver.try_recv() {
            Ok(outcome) => {
                self.status = Some(match &outcome {
                    Ok(report) => report.summary(),
                    Err(message) => message.clone(),
                });
                self.session.finish_skill_install(outcome);
                self.skill_receiver = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.session
                    .finish_skill_install(Err("Installation stopped unexpectedly.".to_string()));
                self.skill_receiver = None;
                true
            }
        }
    }

    /// Sets the agent skill publisher this surface offers.
    pub fn set_skills(&mut self, skills: Option<SkillPublisher>) {
        self.session.set_skills(skills);
    }

    /// Returns a one-line summary of the run state, for the status bar.
    pub fn run_summary(&self) -> String {
        match self.session.run_state() {
            RunState::Idle => String::new(),
            RunState::Running {
                fraction, chunks, ..
            } => {
                let progress = match fraction {
                    Some(value) => format!("Working… {}%", (value * 100.0).round() as i64),
                    None => "Working…".to_string(),
                };
                if chunks.is_empty() {
                    progress
                } else {
                    format!("{progress}  {} results so far", chunks.len())
                }
            }
            RunState::Succeeded { .. } => "Done".to_string(),
            RunState::Failed { message, .. } => message.clone(),
            RunState::Cancelled => "Stopped before it finished.".to_string(),
        }
    }
}
