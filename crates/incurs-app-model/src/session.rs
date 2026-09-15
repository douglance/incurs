//! The interaction state of one interactive surface.
//!
//! [`AppSession`] holds everything a window or a terminal needs to know and
//! nothing about how to draw it: which commands exist, which is selected, the
//! collected values, and the state of the current call. A view owns its own
//! controls and its own event pump, and drives this.
//!
//! The split matters for more than sharing. The rules that decide what a person
//! sees after a call — that cancellation wins over a late success, that a
//! streaming command reports its chunks as the result — used to live inside a
//! method taking a window context, so they could only be tested by opening a
//! window. Here they are ordinary functions over ordinary data.

use std::collections::BTreeMap;
use std::sync::Arc;

use incurs::output::CtaBlock;
use incurs::tool::{ToolCallOutcome, ToolDefinition, ToolEvent};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::form::{FieldIssue, FieldKind, FormModel, FormState};
use crate::runtime::{RunHandle, RunUpdate, ToolRunner};
use crate::skills::{SkillPublisher, SkillReport};
use crate::text::humanize;

/// How many progress and log lines are retained for display.
const LOG_LIMIT: usize = 200;

/// One command as an interactive surface presents it.
#[derive(Debug, Clone)]
pub struct CommandItem {
    /// The exposed tool name used for invocation.
    pub name: String,
    /// The label shown in a command list.
    pub title: String,
    /// The command's neutral summary.
    pub description: String,
    /// Whether the contract marks the command as destructive.
    pub destructive: bool,
    /// The lowered input form.
    pub model: FormModel,
}

impl CommandItem {
    /// Builds a presentation item from one tool definition.
    pub fn from_definition(definition: &ToolDefinition) -> Self {
        let title = definition
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.title.clone())
            .unwrap_or_else(|| humanize(&definition.name));
        let destructive = definition
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.destructive_hint)
            .unwrap_or(false);

        Self {
            name: definition.name.clone(),
            title,
            description: definition.description.clone(),
            destructive,
            model: FormModel::from_input_schema(&definition.input_schema),
        }
    }
}

/// The state of the current or most recent call.
#[derive(Debug, Clone)]
pub enum RunState {
    /// No call has been made for the selected command.
    Idle,
    /// A call is in flight.
    Running {
        /// Cancellation signal for the active call.
        cancellation: CancellationToken,
        /// Progress messages and logs, newest last.
        log: Vec<String>,
        /// The most recent reported completion fraction.
        fraction: Option<f64>,
        /// Streamed chunks collected so far.
        ///
        /// Unbounded, as in the view this was lifted from. Capping it is a
        /// behavior change and belongs in its own commit, not a move.
        chunks: Vec<Value>,
    },
    /// The call succeeded.
    Succeeded {
        /// The structured result.
        data: Value,
        /// Follow-up suggestions reported by the command.
        cta: Option<CtaBlock>,
    },
    /// The call failed.
    Failed {
        /// The human-readable failure message.
        message: String,
        /// The machine-readable failure code.
        code: String,
        /// Problems that matched no field.
        details: Vec<String>,
    },
    /// The call was cancelled by the person using the application.
    Cancelled,
}

/// The state of agent skill installation.
#[derive(Debug, Clone)]
pub enum SkillState {
    /// Nothing has been installed in this session.
    Idle,
    /// An installation is in flight.
    Installing,
    /// The last installation succeeded.
    Installed(SkillReport),
    /// The last installation failed.
    Failed(String),
}

/// The interaction state of one interactive surface.
pub struct AppSession {
    runner: Arc<ToolRunner>,
    version: Option<String>,
    commands: Vec<CommandItem>,
    selected: Option<usize>,
    state: FormState,
    run: RunState,
    skills: Option<SkillPublisher>,
    skill_state: SkillState,
}

impl AppSession {
    /// Builds a session over one runner's catalog, selecting the first command.
    pub fn new(runner: Arc<ToolRunner>) -> Self {
        let (version, commands) = {
            let catalog = runner.catalog();
            let version = catalog.version().map(ToString::to_string);
            let commands: Vec<CommandItem> = catalog
                .definitions()
                .iter()
                .map(CommandItem::from_definition)
                .collect();
            (version, commands)
        };

        let mut session = Self {
            runner,
            version,
            commands,
            selected: None,
            state: FormState::default(),
            run: RunState::Idle,
            skills: None,
            skill_state: SkillState::Idle,
        };
        if !session.commands.is_empty() {
            session.select(0);
        }
        session
    }

    /// Returns the catalog version, when the application declares one.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Returns every command, in stable order.
    pub fn commands(&self) -> &[CommandItem] {
        &self.commands
    }

    /// Returns the indices of commands matching one search query.
    pub fn visible(&self, query: &str) -> Vec<usize> {
        let query = query.trim().to_lowercase();
        self.commands
            .iter()
            .enumerate()
            .filter(|(_, command)| {
                query.is_empty()
                    || command.title.to_lowercase().contains(&query)
                    || command.name.to_lowercase().contains(&query)
                    || command.description.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Selects one command by index and reseeds its values.
    pub fn select(&mut self, index: usize) {
        let Some(command) = self.commands.get(index) else {
            return;
        };
        self.selected = Some(index);
        self.state = FormState::new(&command.model);
        self.run = RunState::Idle;
    }

    /// Selects the command with the given exposed tool name.
    ///
    /// Returns whether a command with that name exists.
    pub fn select_command(&mut self, name: &str) -> bool {
        match self
            .commands
            .iter()
            .position(|command| command.name == name)
        {
            Some(index) => {
                self.select(index);
                true
            }
            None => false,
        }
    }

    /// Returns the index of the selected command.
    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// Returns the selected command.
    pub fn selected(&self) -> Option<&CommandItem> {
        self.selected.and_then(|index| self.commands.get(index))
    }

    /// Returns the selected command's exposed tool name.
    pub fn selected_command(&self) -> Option<&str> {
        self.selected().map(|command| command.name.as_str())
    }

    /// Returns the selected command's form model.
    pub fn form(&self) -> Option<&FormModel> {
        self.selected().map(|command| &command.model)
    }

    /// Returns the collected values.
    pub fn state(&self) -> &FormState {
        &self.state
    }

    /// Returns the collected values for modification.
    ///
    /// This is how a view drives a toggle or a choice, which have no text
    /// buffer of their own.
    pub fn state_mut(&mut self) -> &mut FormState {
        &mut self.state
    }

    /// Returns each text-like field with the placeholder its control should show.
    ///
    /// A view creates one editor per entry, seeded from [`AppSession::state`].
    pub fn text_fields(&self) -> Vec<(String, String)> {
        let Some(command) = self.selected() else {
            return Vec::new();
        };
        command
            .model
            .fields
            .iter()
            .filter(|field| field.kind.is_text_like())
            .map(|field| {
                let placeholder = match &field.kind {
                    FieldKind::List => "Separate entries with commas".to_string(),
                    FieldKind::Json => "JSON value".to_string(),
                    FieldKind::Number { integer: true } => "Whole number".to_string(),
                    FieldKind::Number { integer: false } => "Number".to_string(),
                    _ => field.label.clone(),
                };
                (field.name.clone(), placeholder)
            })
            .collect()
    }

    /// Starts the selected command with the collected values.
    ///
    /// The caller pumps the returned handle and feeds each update to
    /// [`AppSession::receive`]. A view must write its own text buffers into
    /// [`AppSession::state_mut`] before calling this; the controls are the
    /// source of truth for text, not the form state.
    ///
    /// # Errors
    ///
    /// Returns the per-field coercion problems, which are also attached to the
    /// fields themselves, when a value cannot be sent as its declared type.
    pub fn start_run(&mut self) -> Result<RunHandle, Vec<FieldIssue>> {
        let Some(command) = self.selected() else {
            return Err(Vec::new());
        };
        let name = command.name.clone();
        let model = command.model.clone();

        self.state.clear_issues();
        let arguments = match self.state.arguments(&model) {
            Ok(arguments) => arguments,
            Err(issues) => {
                for issue in &issues {
                    self.state.value_mut(&issue.field).issue = Some(issue.message.clone());
                }
                return Err(issues);
            }
        };

        let handle = self.runner.start(&name, arguments);
        self.run = RunState::Running {
            cancellation: handle.cancellation.clone(),
            log: Vec::new(),
            fraction: None,
            chunks: Vec::new(),
        };
        Ok(handle)
    }

    /// Applies one ordered update from the running call.
    pub fn receive(&mut self, update: RunUpdate) {
        match update {
            RunUpdate::Event(event) => {
                if let RunState::Running {
                    log,
                    fraction,
                    chunks,
                    ..
                } = &mut self.run
                {
                    match event {
                        ToolEvent::Progress {
                            message,
                            fraction: reported,
                        } => {
                            log.push(message);
                            if reported.is_some() {
                                *fraction = reported;
                            }
                        }
                        ToolEvent::Log { level, message } => {
                            log.push(format!("{level}: {message}"));
                        }
                        ToolEvent::Chunk { data } => chunks.push(data),
                    }
                    if log.len() > LOG_LIMIT {
                        log.drain(0..log.len() - LOG_LIMIT);
                    }
                }
            }
            RunUpdate::Finished(outcome) => self.finish(*outcome),
        }
    }

    /// Records the terminal outcome of a call.
    fn finish(&mut self, outcome: ToolCallOutcome) {
        let cancelled = matches!(
            &self.run,
            RunState::Running { cancellation, .. } if cancellation.is_cancelled()
        );
        let streamed = match &self.run {
            RunState::Running { chunks, .. } if !chunks.is_empty() => Some(chunks.clone()),
            _ => None,
        };

        self.run = match outcome {
            ToolCallOutcome::Ok { data, cta } => {
                if cancelled {
                    RunState::Cancelled
                } else {
                    RunState::Succeeded {
                        // A streaming command reports its chunks as the result,
                        // because its terminal value carries no rows.
                        data: match streamed {
                            Some(chunks) if data.is_null() => Value::Array(chunks),
                            _ => data,
                        },
                        cta,
                    }
                }
            }
            ToolCallOutcome::Error {
                code,
                message,
                field_errors,
                ..
            } => {
                if cancelled {
                    RunState::Cancelled
                } else {
                    let details = field_errors
                        .as_ref()
                        .map(|errors| {
                            let model = self
                                .selected()
                                .map(|command| command.model.clone())
                                .unwrap_or_default();
                            self.state.apply_field_errors(&model, errors)
                        })
                        .unwrap_or_default();
                    RunState::Failed {
                        message,
                        code,
                        details,
                    }
                }
            }
        };
    }

    /// Requests cancellation of the active call, if any.
    ///
    /// The command still reports a terminal outcome, so a surface always
    /// observes a [`RunUpdate::Finished`] and never leaves a call pending.
    pub fn cancel(&mut self) {
        if let RunState::Running { cancellation, .. } = &self.run {
            cancellation.cancel();
        }
    }

    /// Returns the state of the current or most recent call.
    pub fn run_state(&self) -> &RunState {
        &self.run
    }

    /// Returns whether a call is in flight.
    pub fn is_running(&self) -> bool {
        matches!(self.run, RunState::Running { .. })
    }

    /// Returns the most recent successful result.
    pub fn last_result(&self) -> Option<&Value> {
        match &self.run {
            RunState::Succeeded { data, .. } => Some(data),
            _ => None,
        }
    }

    /// Returns the most recent failure message.
    pub fn last_error(&self) -> Option<&str> {
        match &self.run {
            RunState::Failed { message, .. } => Some(message),
            _ => None,
        }
    }

    /// Sets the agent skill publisher offered by this surface.
    pub fn set_skills(&mut self, skills: Option<SkillPublisher>) {
        self.skills = skills;
    }

    /// Returns the configured agent skill publisher.
    pub fn skills(&self) -> Option<&SkillPublisher> {
        self.skills.as_ref()
    }

    /// Returns the state of agent skill installation.
    pub fn skill_state(&self) -> &SkillState {
        &self.skill_state
    }

    /// Returns the publisher to install, marking installation as started.
    ///
    /// Returns `None` when no publisher is configured or one is already
    /// running. The caller performs the installation and reports the outcome
    /// to [`AppSession::finish_skill_install`].
    pub fn begin_skill_install(&mut self) -> Option<SkillPublisher> {
        let publisher = self.skills.clone()?;
        if matches!(self.skill_state, SkillState::Installing) {
            return None;
        }
        self.skill_state = SkillState::Installing;
        Some(publisher)
    }

    /// Records the outcome of an installation.
    pub fn finish_skill_install(&mut self, outcome: Result<SkillReport, String>) {
        self.skill_state = match outcome {
            Ok(report) => SkillState::Installed(report),
            Err(message) => SkillState::Failed(message),
        };
    }

    /// Returns the runner, for a surface that must spawn its own work.
    pub fn runner(&self) -> &Arc<ToolRunner> {
        &self.runner
    }

    /// Returns the collected arguments without starting a call.
    ///
    /// # Errors
    ///
    /// Returns the per-field coercion problems.
    pub fn arguments(&self) -> Result<BTreeMap<String, Value>, Vec<FieldIssue>> {
        let model = self
            .selected()
            .map(|command| command.model.clone())
            .unwrap_or_default();
        self.state.arguments(&model)
    }
}
