//! The interaction state machine, driven without a window or a terminal.
//!
//! These rules decide what a person sees after a call. Until this crate existed
//! they lived inside a method taking a window context, so the only way to reach
//! them was to open a window — which meant a toolkit, a display, and a frame
//! loop for logic that is ordinary data.
//!
//! They are ordinary `#[test]` functions, not async ones. That is not an
//! accident: every transition here is synchronous, and a `ToolRunner` owns a
//! Tokio runtime that panics if it is dropped inside an async context.

use std::collections::HashMap;
use std::sync::Arc;

use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler};
use incurs::output::CommandResult;
use incurs::schema::{FieldMeta, FieldType};
use incurs::tool::ToolCallOutcome;
use incurs_app_model::{AppSession, CallEnvironment, RunState, RunUpdate, ToolRunner};
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

/// Builds one field description.
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

/// A catalog with one command carrying an argument, a choice, and a switch.
fn session() -> AppSession {
    let command = CommandDef {
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

    let cli = Cli::create("things")
        .version("9.9.9")
        .command("add", command);
    let runner = ToolRunner::new(
        cli.try_tool_catalog().expect("unique tool names"),
        CallEnvironment::Isolated,
    )
    .expect("runtime");

    AppSession::new(Arc::new(runner))
}

#[test]
fn a_session_selects_its_first_command_and_lowers_the_form() {
    let session = session();

    assert_eq!(session.selected_command(), Some("add"));
    assert_eq!(session.version(), Some("9.9.9"));
    let names: Vec<&str> = session
        .form()
        .expect("a command is selected")
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert!(names.contains(&"title"));
    // The required argument is placed first, which is what puts positionals
    // ahead of options on every surface.
    assert_eq!(names.first(), Some(&"title"));
}

/// A toggle and a choice can be driven with no view at all.
///
/// The desktop view could only set text; a switch or a choice was reachable
/// only by clicking one. That is what made a second surface impossible.
#[test]
fn a_switch_and_a_choice_are_drivable_without_a_view() {
    let mut session = session();

    session.state_mut().value_mut("done").toggle = true;
    session.state_mut().value_mut("priority").choice = "high".to_string();
    session.state_mut().value_mut("title").text = "Buy milk".to_string();

    let arguments = session.arguments().expect("values coerce");
    assert_eq!(arguments["title"], json!("Buy milk"));
    assert_eq!(arguments["priority"], json!("high"));
    assert_eq!(arguments["done"], json!(true));
}

/// Cancelling wins over a success that arrives afterwards.
#[test]
fn a_cancelled_call_reports_cancelled_even_if_it_then_succeeds() {
    let mut session = session();
    session.state_mut().value_mut("title").text = "Buy milk".to_string();

    let _handle = session.start_run().expect("the call starts");
    session.cancel();
    session.receive(RunUpdate::Finished(Box::new(ToolCallOutcome::Ok {
        data: json!({ "title": "Buy milk" }),
        cta: None,
    })));

    assert!(
        matches!(session.run_state(), RunState::Cancelled),
        "a result arriving after cancellation must not be shown as success"
    );
}

/// A streaming command reports its chunks, because its final value has none.
#[test]
fn a_streaming_command_reports_its_chunks_as_the_result() {
    let mut session = session();
    session.state_mut().value_mut("title").text = "Buy milk".to_string();

    let _handle = session.start_run().expect("the call starts");
    for index in 0..3 {
        session.receive(RunUpdate::Event(incurs::tool::ToolEvent::Chunk {
            data: json!({ "index": index }),
        }));
    }
    session.receive(RunUpdate::Finished(Box::new(ToolCallOutcome::Ok {
        data: Value::Null,
        cta: None,
    })));

    assert_eq!(
        session.last_result(),
        Some(&json!([{ "index": 0 }, { "index": 1 }, { "index": 2 }])),
        "a null terminal value with chunks shows the chunks"
    );
}

/// A terminal value that carries rows is shown instead of the chunks.
#[test]
fn a_final_value_wins_over_collected_chunks() {
    let mut session = session();
    session.state_mut().value_mut("title").text = "Buy milk".to_string();

    let _handle = session.start_run().expect("the call starts");
    session.receive(RunUpdate::Event(incurs::tool::ToolEvent::Chunk {
        data: json!({ "index": 0 }),
    }));
    session.receive(RunUpdate::Finished(Box::new(ToolCallOutcome::Ok {
        data: json!({ "total": 1 }),
        cta: None,
    })));

    assert_eq!(session.last_result(), Some(&json!({ "total": 1 })));
}

/// Progress and log events accumulate while a call runs.
#[test]
fn progress_and_logs_accumulate_while_running() {
    let mut session = session();
    session.state_mut().value_mut("title").text = "Buy milk".to_string();

    let _handle = session.start_run().expect("the call starts");
    session.receive(RunUpdate::Event(incurs::tool::ToolEvent::Progress {
        message: "Working".to_string(),
        fraction: Some(0.5),
    }));
    session.receive(RunUpdate::Event(incurs::tool::ToolEvent::Log {
        level: "warn".to_string(),
        message: "Slow".to_string(),
    }));

    let RunState::Running { log, fraction, .. } = session.run_state() else {
        panic!("the call should still be running");
    };
    assert_eq!(log, &["Working".to_string(), "warn: Slow".to_string()]);
    assert_eq!(*fraction, Some(0.5));
}

/// A runtime field error attaches to the field it names.
#[test]
fn a_field_error_attaches_to_its_field() {
    let mut session = session();
    session.state_mut().value_mut("title").text = "Buy milk".to_string();

    let _handle = session.start_run().expect("the call starts");
    session.receive(RunUpdate::Finished(Box::new(ToolCallOutcome::Error {
        code: "VALIDATION_ERROR".to_string(),
        message: "Check your input.".to_string(),
        retryable: None,
        field_errors: Some(vec![incurs::output::FieldErrorOutput {
            path: "args.title".to_string(),
            message: "Too short.".to_string(),
            expected: "a longer title".to_string(),
            received: "Buy milk".to_string(),
        }]),
        cta: None,
        exit_code: Some(1),
    })));

    assert_eq!(session.last_error(), Some("Check your input."));
    assert_eq!(
        session
            .state()
            .value("title")
            .and_then(|value| value.issue.as_deref()),
        Some("Too short."),
        "the problem must be shown beside the field it is about"
    );
}

#[test]
fn selecting_an_unknown_command_reports_it() {
    let mut session = session();

    assert!(!session.select_command("nope"));
    assert_eq!(
        session.selected_command(),
        Some("add"),
        "a failed selection leaves the previous one in place"
    );
}

#[test]
fn search_matches_name_title_and_description() {
    let session = session();

    assert_eq!(session.visible("").len(), 1);
    assert_eq!(session.visible("add").len(), 1);
    assert_eq!(session.visible("thing").len(), 1, "matches the description");
    assert!(session.visible("zzz").is_empty());
}
