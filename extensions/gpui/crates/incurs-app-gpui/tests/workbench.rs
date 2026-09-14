//! Headless tests that render the real workbench against a real catalog.
//!
//! These drive the same path a person does: build a command graph, open a
//! window, draw the view, and run a command through the shared runtime. They
//! use GPUI's test platform, so they need no display and prove more than a
//! screenshot does — that the view lays out, and that a call reaches the
//! command and comes back into the UI.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{TestAppContext, VisualTestContext};
use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler};
use incurs::output::CommandResult;
use incurs::schema::{FieldMeta, FieldType};
use incurs_app_gpui::runtime::{CallEnvironment, ToolRunner};
use incurs_app_gpui::{SkillPublisher, SkillScope, Workbench, text_field};
use serde_json::{Value, json};

/// Echoes its inputs so a test can observe what reached the command.
struct EchoHandler;

#[async_trait::async_trait]
impl CommandHandler for EchoHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let title = ctx
            .args
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if title.is_empty() {
            return CommandResult::Error {
                code: "EMPTY_TITLE".to_string(),
                message: "Give it a title.".to_string(),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }
        CommandResult::Ok {
            data: json!({
                "title": title,
                "priority": ctx.options.get("priority").cloned().unwrap_or(Value::Null),
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

/// Builds a CLI with one echo command that has an argument and a choice.
fn build_cli() -> Cli {
    let command = CommandDef {
        name: "add".to_string(),
        description: Some("Add a thing".to_string()),
        args_fields: vec![field("title", FieldType::String, true)],
        options_fields: vec![FieldMeta {
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
        }],
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
        .command("add", command)
}

/// Opens a window holding the workbench for the test CLI.
fn open(cx: &mut TestAppContext) -> (gpui::Entity<Workbench>, &mut VisualTestContext) {
    cx.update(text_field::bind_keys);

    let runner = Arc::new(
        ToolRunner::new(
            build_cli().try_tool_catalog().expect("unique tool names"),
            CallEnvironment::Isolated,
        )
        .expect("runtime"),
    );

    cx.add_window_view(|window, cx| Workbench::new(runner, "Things".into(), window, cx))
}

#[gpui::test]
async fn the_workbench_selects_a_command_and_builds_its_form(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx);
    cx.run_until_parked();

    // Opening the window draws the root view. A layout panic or a malformed
    // element tree fails here rather than silently producing a blank window.
    workbench.read_with(cx, |workbench, _| {
        assert_eq!(workbench.selected_command(), Some("add"));
        assert!(!workbench.is_running());
        assert!(workbench.last_result().is_none());
    });
}

#[gpui::test]
async fn running_a_command_reports_its_result_in_the_view(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx);
    cx.run_until_parked();

    workbench.update(cx, |workbench, cx| {
        workbench.set_field_text("title", "Buy milk", cx);
        workbench.run_selected(cx);
    });

    let result = settle(&workbench, cx, |workbench| workbench.last_result().cloned())
        .unwrap_or_else(|| {
            let error = workbench.read_with(&*cx, |workbench, _| {
                workbench.last_error().map(str::to_string)
            });
            panic!("the command should report a result; it reported error: {error:?}")
        });

    // The typed value reached the command, and its default option was applied
    // by the shared runtime rather than by the form.
    assert_eq!(result["title"], json!("Buy milk"));
    assert_eq!(result["priority"], json!("medium"));
}

#[gpui::test]
async fn a_command_failure_is_reported_rather_than_swallowed(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx);
    cx.run_until_parked();

    // Leaving the required argument empty makes the runtime reject the call,
    // which proves the failure path comes from the command runtime and not
    // from a rule invented by the form.
    workbench.update(cx, |workbench, cx| workbench.run_selected(cx));

    let message = settle(&workbench, cx, |workbench| {
        workbench.last_error().map(str::to_string)
    })
    .expect("the failing command should report a message");

    assert!(
        !message.is_empty(),
        "a failure must carry a message a person can read"
    );
    assert!(workbench.read_with(cx, |workbench, _| workbench.last_result().is_none()));
}

#[gpui::test]
async fn text_typed_into_a_control_reaches_the_command(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx);
    cx.run_until_parked();

    // Type into the focused control rather than setting state directly, so
    // this covers the path from keystroke to argument.
    cx.update(|window, cx| {
        workbench.update(cx, |workbench, cx| {
            assert!(workbench.focus_field("title", window, cx));
        });
    });
    cx.simulate_input("Buy milk");
    workbench.update(cx, |workbench, cx| workbench.run_selected(cx));

    let result = settle(&workbench, cx, |workbench| workbench.last_result().cloned())
        .unwrap_or_else(|| {
            let error = workbench.read_with(&*cx, |workbench, _| {
                workbench.last_error().map(str::to_string)
            });
            panic!("typing should reach the command; it reported error: {error:?}")
        });

    assert_eq!(result["title"], json!("Buy milk"));
}

#[gpui::test]
async fn the_skills_button_installs_through_the_view(cx: &mut TestAppContext) {
    let (workbench, cx) = open(cx);
    cx.run_until_parked();

    // Scope the install to a temporary project directory so the test proves
    // the real publisher path without writing to the account's agent config.
    let root = std::env::temp_dir().join(format!(
        "incurs-app-gpui-view-skills-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch directory");

    workbench.update(cx, |workbench, cx| {
        workbench.set_skills(
            Some(
                SkillPublisher::from_cli(&build_cli())
                    .scope(SkillScope::Project)
                    .directory(&root),
            ),
            cx,
        );
        workbench.install_skills(cx);
    });

    let report = settle(&workbench, cx, |workbench| {
        workbench.last_skill_report().cloned()
    })
    .unwrap_or_else(|| {
        let error = workbench.read_with(&*cx, |workbench, _| {
            workbench.last_skill_error().map(str::to_string)
        });
        panic!("the view should report an install; it reported error: {error:?}")
    });

    assert!(report.installed > 0, "at least one skill should install");
    assert!(
        root.join(".agents").join("skills").is_dir(),
        "the install must reach the filesystem, not just the view"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Runs the window until `read` yields a value, or fails the test.
///
/// A call crosses onto the Tokio runtime and back, so the view settles over
/// several frames rather than on the first one.
fn settle<T>(
    workbench: &gpui::Entity<Workbench>,
    cx: &mut VisualTestContext,
    read: impl Fn(&Workbench) -> Option<T>,
) -> Option<T> {
    for _ in 0..500 {
        cx.run_until_parked();
        if let Some(value) = workbench.read_with(&*cx, |workbench, _| read(workbench)) {
            return Some(value);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    None
}
