//! A command a CLI defines must win over a builtin of the same name.

use incurs::cli::Cli;
use incurs::command::{CommandDef, TypedResult};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(JsonSchema, Serialize)]
struct Handled {
    handled: bool,
}

/// Builds a CLI whose only command shadows one builtin name.
fn shadowing(builtin: &str) -> Cli {
    let command = CommandDef::typed::<(), (), (), Handled, _, _>(builtin, |_| async move {
        TypedResult::ok(Handled { handled: true })
    })
    .description("A command that shadows a builtin")
    .done();

    Cli::create("app").command(builtin, command)
}

/// `completions`, `mcp`, `plugin` and `skills` are builtin command names.
///
/// Their dispatchers used to claim the name unconditionally, and to treat an
/// unrecognized subcommand as success. A CLI that defined its own `plugin`
/// command therefore saw builtin help and exit zero — no diagnostic, no way to
/// reach the handler it had registered, and a caller that could not tell the
/// difference between "ran" and "silently did nothing".
#[tokio::test]
async fn a_defined_command_wins_over_a_builtin_of_the_same_name() {
    for builtin in ["completions", "mcp", "plugin", "skills"] {
        let mut output = Vec::new();
        let exit = shadowing(builtin)
            .serve_to(
                vec![builtin.to_string(), "--json".to_string()],
                &mut output,
                false,
            )
            .await
            .expect("serve_to should not return Err");
        let stdout = String::from_utf8(output).expect("valid UTF-8");

        assert_eq!(exit, None, "`{builtin}` should have run, got: {stdout}");
        assert!(
            stdout.contains("handled"),
            "`{builtin}` must reach the registered handler, got: {stdout}"
        );
    }
}

/// A shadowing command is also reachable through the non-CLI boundary.
#[tokio::test]
async fn a_shadowing_command_is_callable_as_a_tool() {
    let catalog = shadowing("plugin").tool_catalog();

    assert!(
        catalog.get("plugin").is_some(),
        "a defined command must reach MCP and Code Mode too"
    );
}

/// A builtin stays reachable when nothing shadows it.
#[tokio::test]
async fn an_unshadowed_builtin_still_runs() {
    let mut output = Vec::new();
    Cli::create("app")
        .serve_to(vec!["skills".to_string()], &mut output, false)
        .await
        .expect("serve_to should not return Err");

    let stdout = String::from_utf8(output).expect("valid UTF-8");
    assert!(
        stdout.contains("skills"),
        "the builtin must still produce its own output, got: {stdout}"
    );
}
