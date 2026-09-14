//! Golden tests pinning the observable CLI surface.
//!
//! These replace the cross-implementation parity gate that compared incurs
//! against the vendored TypeScript oracle. Every case here was captured from a
//! tree where `NODE_NO_WARNINGS=1 cargo xtask parity` passed, so the goldens
//! record behavior the oracle agreed with rather than merely whatever the code
//! did on the day they were written.
//!
//! Each golden records the exit code and stdout together. The parity gate also
//! compared stderr; for this fixture stderr is empty by construction, because
//! `todoapp`'s logging middleware writes only when `!ctx.agent` and these runs
//! are all in agent mode. `harness_matches_a_real_process` proves the
//! in-process harness reproduces what spawning the binary observes, so the
//! goldens do not quietly lose that coverage.
//!
//! Regenerate after an intentional change:
//!
//! ```sh
//! UPDATE_GOLDEN=1 cargo test -p incurs --test cli_surface --all-features
//! ```
//!
//! Review the resulting diff as the wire-format change it is.

use std::path::PathBuf;

use incurs::cli::Cli;

#[allow(dead_code)]
#[path = "../examples/todoapp.rs"]
mod todoapp;

/// Runs one argv against the fixture and renders the full observation.
///
/// Uses agent mode, matching a piped process, which is how the parity gate
/// invoked both implementations.
async fn observe(argv: &[&str]) -> String {
    let cli: Cli = todoapp::build_cli();
    let argv: Vec<String> = argv.iter().map(|token| token.to_string()).collect();

    let mut buffer = Vec::new();
    let exit_code = cli
        .serve_to(argv, &mut buffer, false)
        .await
        .expect("serve_to should not return Err");
    let stdout = String::from_utf8(buffer).expect("output should be valid UTF-8");

    render(exit_code, &strip_durations(&stdout))
}

/// Renders one observation in the golden file format.
fn render(exit_code: Option<i32>, stdout: &str) -> String {
    let exit = match exit_code {
        Some(code) => code.to_string(),
        None => "none".to_string(),
    };
    format!("exit: {exit}\n--- stdout ---\n{stdout}")
}

/// Replaces measured durations so goldens stay stable across runs.
///
/// Mirrors the normalization the parity gate applied before comparing.
fn strip_durations(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find(|c: char| c.is_ascii_digit()) {
        let (head, tail) = rest.split_at(start);
        let digits: String = tail
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let after = &tail[digits.len()..];
        if let Some(remainder) = after.strip_prefix("ms") {
            output.push_str(head);
            output.push_str("<duration>ms");
            rest = remainder;
        } else {
            output.push_str(head);
            output.push_str(&digits);
            rest = after;
        }
    }
    output.push_str(rest);
    output
}

/// Returns the path of one golden file.
fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("cli_surface")
        .join(format!("{}.txt", name.replace(' ', "-")))
}

/// Compares one observation against its golden, or rewrites it on request.
fn assert_golden(name: &str, observed: &str) {
    let path = golden_path(name);

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("golden directory"))
            .expect("golden directory should be creatable");
        std::fs::write(&path, observed).expect("golden should be writable");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "missing golden {}: {error}\nrerun with UPDATE_GOLDEN=1 to create it",
            path.display()
        )
    });

    assert_eq!(
        observed,
        expected,
        "the observable CLI surface changed for case `{name}`.\n\
         If that is intended, rerun with UPDATE_GOLDEN=1 and review the diff."
    );
}

/// Declares one golden case.
macro_rules! surface_case {
    ($test:ident, $name:literal, $argv:expr) => {
        #[tokio::test]
        async fn $test() {
            assert_golden($name, &observe(&$argv).await);
        }
    };
}

// The sixteen cases ported from `tests/parity/cases.json`.
surface_case!(help, "help", ["--help"]);
surface_case!(version, "version", ["--version"]);
surface_case!(default_toon, "default-toon", ["stats"]);
surface_case!(json_shorthand, "json-shorthand", ["stats", "--json"]);
surface_case!(json_format, "json-format", ["list", "--format", "json"]);
surface_case!(option_default, "option-default", ["list", "--json"]);
surface_case!(
    option_value,
    "option-value",
    ["list", "--status", "pending", "--json"]
);
surface_case!(option_alias, "option-alias", ["list", "-s", "done", "--json"]);
surface_case!(positional, "positional", ["get", "1", "--json"]);
surface_case!(structured_error, "structured-error", ["get", "99", "--json"]);
surface_case!(
    cta,
    "cta",
    ["add", "test item", "--priority", "high", "--json"]
);
surface_case!(
    filter,
    "filter",
    ["stats", "--filter-output", "by_priority.high", "--json"]
);
surface_case!(
    full_output,
    "full-output",
    ["complete", "1", "--full-output", "--json"]
);
surface_case!(buffered_stream, "buffered-stream", ["stream", "--json"]);
surface_case!(
    jsonl_stream,
    "jsonl-stream",
    ["stream", "--format", "jsonl"]
);

// `--format yaml` had no coverage anywhere before this file.
#[cfg(feature = "yaml")]
surface_case!(yaml_format, "yaml-format", ["stats", "--format", "yaml"]);

// Schema-shaped surfaces. These are not parity cases; they are pinned here
// because the Capability Interface Catalog migration must change them, and a
// golden that must be updated makes a wire-format change impossible to land
// without review.
surface_case!(schema_command, "schema-command", ["add", "--schema"]);
surface_case!(llms_index, "llms-index", ["--llms"]);
surface_case!(llms_full, "llms-full", ["--llms-full", "--format", "json"]);

/// The in-process harness must observe what spawning the binary observes.
///
/// The goldens record stdout and the exit code only. This proves that is the
/// whole observation for this fixture: a real process writes nothing to stderr
/// in agent mode, so nothing is being silently dropped.
#[tokio::test]
async fn harness_matches_a_real_process() {
    let executable = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/examples/todoapp");
    if !executable.is_file() {
        // The example is built by `cargo test --all-targets`; skip rather than
        // fail when this test runs against a tree that has not built it.
        eprintln!("skipping: {} is not built", executable.display());
        return;
    }

    for argv in [
        vec!["stats", "--json"],
        vec!["get", "99", "--json"],
        vec!["--help"],
    ] {
        let output = std::process::Command::new(&executable)
            .args(&argv)
            .output()
            .expect("the example binary should run");

        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "",
            "a piped run writes nothing to stderr, so the goldens lose no coverage by \
             recording stdout alone (argv: {argv:?})"
        );

        let spawned = render(
            output.status.code().filter(|code| *code != 0),
            &strip_durations(&String::from_utf8_lossy(&output.stdout)),
        );
        assert_eq!(
            spawned,
            observe(&argv).await,
            "the in-process harness diverged from a real process (argv: {argv:?})"
        );
    }
}
