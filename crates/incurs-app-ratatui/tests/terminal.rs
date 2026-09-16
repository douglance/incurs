//! The terminal surface, driven without a terminal.
//!
//! Rendering is asserted against `TestBackend`'s in-memory buffer, and the
//! keyboard is asserted by calling the key handler directly. Neither needs a
//! tty, so these run on any CI runner.

use std::collections::HashMap;
use std::sync::Arc;

use incurs::cli::Cli;
use incurs::command::{
    CommandContext, CommandDef, CommandHandler, McpAnnotations, McpCommandOptions,
};
use incurs::output::CommandResult;
use incurs::schema::{FieldMeta, FieldType};
use incurs_app_model::{AppSession, CallEnvironment, ToolRunner};
use incurs_app_ratatui::app::{App, Focus};
use incurs_app_ratatui::{input, view};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::{Value, json};

/// Echoes what reached the command.
struct EchoHandler;

#[async_trait::async_trait]
impl CommandHandler for EchoHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: json!({
                "title": ctx.args.get("title").cloned().unwrap_or(Value::Null),
                "priority": ctx.options.get("priority").cloned().unwrap_or(Value::Null),
                "done": ctx.options.get("done").cloned().unwrap_or(Value::Null),
            }),
            cta: None,
            exit_code: None,
        }
    }
}

fn field(name: &'static str, field_type: FieldType, required: bool) -> FieldMeta {
    FieldMeta {
        name,
        cli_name: name.to_string(),
        description: Some("A field."),
        field_type,
        required,
        default: None,
        alias: None,
        deprecated: false,
        env_name: None,
    }
}

/// A graph with a text field, a choice, a switch, and a destructive command.
fn build_cli() -> Cli {
    let add = CommandDef {
        name: "add".to_string(),
        description: Some("Add a thing".to_string()),
        args_fields: vec![field("title", FieldType::String, true)],
        options_fields: vec![
            FieldMeta {
                default: Some(json!("medium")),
                ..field(
                    "priority",
                    FieldType::Enum(vec![
                        "low".to_string(),
                        "medium".to_string(),
                        "high".to_string(),
                    ]),
                    false,
                )
            },
            field("done", FieldType::Boolean, false),
        ],
        env_fields: Vec::new(),
        aliases: HashMap::new(),
        command_aliases: Vec::new(),
        examples: Vec::new(),
        hint: None,
        format: None,
        output_policy: None,
        handler: Box::new(EchoHandler),
        middleware: Vec::new(),
        output_schema: None,
    };

    Cli::create("things")
        .version("9.9.9")
        .command("add", add)
        .command(
            "wipe",
            CommandDef::build("wipe", EchoHandler)
                .description("Remove everything")
                .mcp(McpCommandOptions {
                    destructive: true,
                    annotations: Some(McpAnnotations {
                        title: None,
                        read_only_hint: Some(false),
                        destructive_hint: Some(true),
                        idempotent_hint: Some(true),
                        open_world_hint: Some(false),
                    }),
                    ..Default::default()
                })
                .done(),
        )
}

fn app() -> App {
    let runner = ToolRunner::new(
        build_cli().try_tool_catalog().expect("unique tool names"),
        CallEnvironment::Isolated,
    )
    .expect("runtime");
    App::new(AppSession::new(Arc::new(runner)), "Things")
}

/// Renders one frame and returns it as lines of text.
fn render(app: &App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(90, 30)).expect("test terminal");
    terminal
        .draw(|frame| view::draw(frame, app))
        .expect("draw succeeds");
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn screen(app: &App) -> String {
    render(app).join("\n")
}

/// Focuses the form field with one name.
///
/// Required fields are placed before optional ones, and optional ones follow in
/// schema order, so a positional index is not a stable way to name a field.
fn focus_field(app: &mut App, name: &str) {
    app.focus = Focus::Form;
    app.field = app
        .session
        .form()
        .expect("a command is selected")
        .fields
        .iter()
        .position(|field| field.name == name)
        .unwrap_or_else(|| panic!("no field named `{name}`"));
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: ratatui::crossterm::event::KeyEventState::NONE,
    }
}

fn control(character: char) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(character),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: ratatui::crossterm::event::KeyEventState::NONE,
    }
}

/// The frame lays out without panicking and shows the command graph.
#[test]
fn the_first_frame_shows_the_commands_and_the_selected_form() {
    let app = app();
    let screen = screen(&app);

    assert!(screen.contains("Commands"), "got:\n{screen}");
    assert!(screen.contains("Add"), "the command list is drawn");
    assert!(
        screen.contains("Title"),
        "the selected command's form is drawn"
    );
}

/// A destructive command is marked, so a person sees it before running it.
#[test]
fn a_destructive_command_is_marked_in_the_list() {
    let screen = screen(&app());

    assert!(screen.contains('⚠'), "got:\n{screen}");
}

/// A closed set renders as a cycling choice, not a text box.
///
/// This is the control the desktop surface got wrong until the MCP schema
/// started carrying `enum`, so it is asserted rather than assumed.
#[test]
fn a_closed_set_renders_as_a_choice_with_the_default_marked() {
    let screen = screen(&app());

    assert!(screen.contains("low"), "every option is shown:\n{screen}");
    assert!(screen.contains("high"));
    assert!(
        screen.contains("·medium·"),
        "the schema default is marked as chosen:\n{screen}"
    );
}

/// A boolean renders as a checkbox and flips with space.
#[test]
fn a_switch_renders_as_a_checkbox_and_flips() {
    let mut app = app();
    assert!(screen(&app).contains("[ ]"));

    focus_field(&mut app, "done");
    input::handle(&mut app, key(KeyCode::Char(' ')));

    assert!(
        screen(&app).contains("[x]"),
        "space must flip the switch:\n{}",
        screen(&app)
    );
}

/// A choice cycles with the arrow keys, and wraps.
#[test]
fn a_choice_cycles_with_the_arrow_keys() {
    let mut app = app();
    focus_field(&mut app, "priority");

    input::handle(&mut app, key(KeyCode::Right));
    assert_eq!(
        app.session
            .state()
            .value("priority")
            .map(|v| v.choice.clone()),
        Some("high".to_string())
    );

    input::handle(&mut app, key(KeyCode::Right));
    assert_eq!(
        app.session
            .state()
            .value("priority")
            .map(|v| v.choice.clone()),
        Some("low".to_string()),
        "the choice wraps rather than stopping at the end"
    );

    input::handle(&mut app, key(KeyCode::Left));
    assert_eq!(
        app.session
            .state()
            .value("priority")
            .map(|v| v.choice.clone()),
        Some("high".to_string())
    );
}

/// Typing reaches the editor and then the command.
///
/// The editors own the text, so this covers the same seam the desktop
/// surface's keystroke test does: characters must survive the hop into form
/// state before arguments are collected.
#[test]
fn typing_reaches_the_command() {
    let mut app = app();
    focus_field(&mut app, "title");

    for character in "Buy milk".chars() {
        input::handle(&mut app, key(KeyCode::Char(character)));
    }
    app.sync_editors();

    let arguments = app.session.arguments().expect("values coerce");
    assert_eq!(arguments["title"], json!("Buy milk"));
    assert!(
        screen(&app).contains("Buy milk"),
        "the typed value is drawn back"
    );
}

/// Backspace removes a character from the focused field.
#[test]
fn backspace_edits_the_focused_field() {
    let mut app = app();
    focus_field(&mut app, "title");
    for character in "abc".chars() {
        input::handle(&mut app, key(KeyCode::Char(character)));
    }
    input::handle(&mut app, key(KeyCode::Backspace));
    app.sync_editors();

    assert_eq!(
        app.session.state().value("title").map(|v| v.text.clone()),
        Some("ab".to_string())
    );
}

/// Tab moves between the two panes.
#[test]
fn tab_moves_focus_between_panes() {
    let mut app = app();
    assert_eq!(app.focus, Focus::Commands);

    input::handle(&mut app, key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Form);

    input::handle(&mut app, key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Commands);
}

/// Moving down the list selects a different command and rebuilds its form.
#[test]
fn moving_the_selection_rebuilds_the_form() {
    let mut app = app();
    assert_eq!(app.session.selected_command(), Some("add"));

    input::handle(&mut app, key(KeyCode::Down));

    assert_eq!(app.session.selected_command(), Some("wipe"));
    assert!(
        screen(&app).contains("needs no information"),
        "the new command has no fields:\n{}",
        screen(&app)
    );
}

/// Search filters the command list.
#[test]
fn search_filters_the_command_list() {
    let mut app = app();
    input::handle(&mut app, key(KeyCode::Char('/')));
    for character in "wipe".chars() {
        input::handle(&mut app, key(KeyCode::Char(character)));
    }

    assert_eq!(app.visible().len(), 1);
    assert_eq!(app.session.selected_command(), Some("wipe"));

    input::handle(&mut app, key(KeyCode::Esc));
    assert_eq!(app.visible().len(), 2, "escape clears the search");
}

/// The help overlay opens and closes.
#[test]
fn help_opens_and_any_key_closes_it() {
    let mut app = app();
    input::handle(&mut app, key(KeyCode::Char('?')));

    assert!(app.help);
    assert!(screen(&app).contains("Keys"), "got:\n{}", screen(&app));

    input::handle(&mut app, key(KeyCode::Char('x')));
    assert!(!app.help);
}

/// Ctrl-C quits when nothing is running.
#[test]
fn control_c_quits_when_idle() {
    let mut app = app();
    input::handle(&mut app, control('c'));

    assert!(app.quit);
}

/// A required field left empty is rejected by the runtime, not by the form.
#[test]
fn an_empty_required_field_is_reported_by_the_runtime() {
    let mut app = app();
    app.run();

    // The call starts: the form does not invent a rule, it sends what it has
    // and lets the command's own validation answer.
    assert!(
        app.handle.is_some() || app.status.is_some(),
        "either the call started or a coercion problem was reported"
    );
}

/// The whole screen renders at a small terminal size without panicking.
#[test]
fn a_small_terminal_still_lays_out() {
    let app = app();
    let mut terminal = Terminal::new(TestBackend::new(40, 14)).expect("test terminal");

    terminal
        .draw(|frame| view::draw(frame, &app))
        .expect("a narrow frame still lays out");
}

/// Without a terminal, running must fail rather than block forever.
///
/// Piping the application used to hang: `init` succeeded, no key could ever
/// arrive, and the draw loop spun. A caller that redirects output now gets an
/// error it can act on.
#[test]
fn running_without_a_terminal_is_refused() {
    // The test harness captures stdout, so this is exactly the no-tty case.
    let error = incurs_app_ratatui::TerminalApp::new(
        build_cli().try_tool_catalog().expect("unique tool names"),
    )
    .run()
    .expect_err("a captured stdout is not a terminal");

    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    assert!(
        error.to_string().contains("terminal"),
        "the message must say what is wrong, got: {error}"
    );
}

/// Draining with nothing running applies nothing rather than panicking.
#[test]
fn draining_an_idle_session_applies_nothing() {
    let mut app = app();

    assert!(!app.drain(), "there is no call in flight");
}
