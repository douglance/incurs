//! Your own terminal interface, built on the hooks.
//!
//! ```sh
//! cargo run -p incurs-app-ratatui --example custom_ui
//! ```
//!
//! This uses none of `TerminalApp`, `App`, `view`, `input`, `editor` or
//! `theme` — it owns its loop, its layout and its widgets, and draws nothing
//! the reference application draws. What it keeps is the contract: calls go
//! through the tool catalog, values are coerced by the form state, and field
//! problems come back from the runtime.
//!
//! It is deliberately ugly and deliberately short. The point is the boundary,
//! not the design.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler};
use incurs::output::CommandResult;
use incurs::schema::{FieldMeta, FieldType};
use incurs_app_model::{AppSession, CallEnvironment, RunHandle, RunState, ToolRunner};
use incurs_app_ratatui::bindings;
use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

/// The store the demo command writes to.
fn counter() -> &'static Mutex<u64> {
    static COUNT: OnceLock<Mutex<u64>> = OnceLock::new();
    COUNT.get_or_init(|| Mutex::new(0))
}

/// Adds one labelled tick.
struct TickHandler;

#[async_trait::async_trait]
impl CommandHandler for TickHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let label = ctx
            .args
            .get("label")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if label.is_empty() {
            return CommandResult::Error {
                code: "EMPTY_LABEL".to_string(),
                message: "Give the tick a label.".to_string(),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }
        let mut count = counter().lock().expect("counter");
        *count += 1;
        CommandResult::Ok {
            data: serde_json::json!({ "label": label, "count": *count }),
            cta: None,
            exit_code: None,
        }
    }
}

fn build_cli() -> Cli {
    let tick = CommandDef {
        name: "tick".to_string(),
        description: Some("Record one labelled tick".to_string()),
        args_fields: vec![FieldMeta {
            name: "label",
            cli_name: "label".to_string(),
            description: Some("What to call it"),
            field_type: FieldType::String,
            required: true,
            default: None,
            alias: None,
            deprecated: false,
            env_name: None,
        }],
        options_fields: Vec::new(),
        env_fields: Vec::new(),
        aliases: HashMap::new(),
        command_aliases: Vec::new(),
        examples: Vec::new(),
        hint: None,
        format: None,
        output_policy: None,
        handler: Box::new(TickHandler),
        middleware: Vec::new(),
        output_schema: None,
    };

    Cli::create("ticker")
        .version(env!("CARGO_PKG_VERSION"))
        .description("A tiny custom interface")
        .command("tick", tick)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Hook one: refuse a terminal that is not one, rather than spinning.
    bindings::require_terminal()?;

    let runner = Arc::new(ToolRunner::new(
        build_cli().try_tool_catalog()?,
        CallEnvironment::Host,
    )?);
    let mut session = AppSession::new(runner);
    let mut running: Option<RunHandle> = None;

    // This interface owns its text, exactly as the reference one does.
    let mut label = String::new();

    let mut terminal = ratatui::init();
    let result = drive(&mut terminal, &mut session, &mut running, &mut label);
    ratatui::restore();
    result.map_err(Into::into)
}

fn drive(
    terminal: &mut ratatui::DefaultTerminal,
    session: &mut AppSession,
    running: &mut Option<RunHandle>,
    label: &mut String,
) -> std::io::Result<()> {
    loop {
        let status = match session.run_state() {
            RunState::Idle => "type a label, enter to run, esc to quit".to_string(),
            RunState::Running { .. } => "working…".to_string(),
            RunState::Succeeded { data, .. } => format!("ok: {data}"),
            RunState::Failed { message, .. } => format!("failed: {message}"),
            RunState::Cancelled => "cancelled".to_string(),
        };
        let problem = session
            .state()
            .value("label")
            .and_then(|value| value.issue.clone())
            .unwrap_or_default();

        terminal.draw(|frame| {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Min(1),
                ])
                .split(frame.area());

            frame.render_widget(
                Paragraph::new(label.as_str())
                    .block(Block::default().borders(Borders::ALL).title(" label ")),
                rows[0],
            );
            frame.render_widget(
                Paragraph::new(Line::from(problem.as_str())).style(Style::default().fg(Color::Red)),
                rows[1],
            );
            frame.render_widget(Paragraph::new(status.as_str()), rows[2]);
        })?;

        if event::poll(std::time::Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Esc => return Ok(()),
                KeyCode::Char(character) => label.push(character),
                KeyCode::Backspace => {
                    label.pop();
                }
                KeyCode::Enter if running.is_none() => {
                    // The contract: write the view's text in, then let the
                    // form coerce it and the runtime validate it.
                    session
                        .state_mut()
                        .set_texts([("label".to_string(), label.clone())]);
                    // A coercion problem is already attached to the field, so
                    // there is nothing to do here but not start a call.
                    if let Ok(handle) = session.start_run() {
                        *running = Some(handle);
                    }
                }
                _ => {}
            }
        }

        // Hook two: collect whatever the call runtime has produced.
        bindings::drain(session, running);
    }
}
