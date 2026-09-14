//! Tests that installing skills actually writes skill files.
//!
//! These install into a temporary project directory rather than the account's
//! home, so they prove the real publisher path without touching the machine's
//! agent configuration.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler};
use incurs::output::CommandResult;
use incurs::schema::{FieldMeta, FieldType};
use incurs_app_gpui::{SkillPublisher, SkillReport, SkillScope};
use serde_json::json;

/// A command that does nothing, since these tests only compile skills.
struct Noop;

#[async_trait::async_trait]
impl CommandHandler for Noop {
    async fn run(&self, _: CommandContext) -> CommandResult {
        CommandResult::Ok {
            data: json!({}),
            cta: None,
            exit_code: None,
        }
    }
}

/// Builds a CLI with two documented commands.
///
/// Each test passes its own name because [`incurs::sync_skills::sync`] stages
/// generated files in a temporary directory keyed by CLI name and process id.
/// Two concurrent syncs sharing a name would share that directory.
fn build_cli(name: &str) -> Cli {
    let command = |name: &str, description: &str| CommandDef {
        name: name.to_string(),
        description: Some(description.to_string()),
        args_fields: vec![FieldMeta {
            name: "title",
            cli_name: "title".to_string(),
            description: Some("What needs doing."),
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
        handler: Box::new(Noop),
        middleware: Vec::new(),
        output_schema: None,
    };

    Cli::create(name)
        .version("1.0.0")
        .description("Keep track of what needs doing")
        .command("add", command("add", "Add a todo"))
        .command("list", command("list", "Show your todos"))
}

/// Creates an empty scratch directory for one test.
fn scratch(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "incurs-app-gpui-skills-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("scratch directory");
    root
}

#[tokio::test]
async fn installing_writes_skill_files_into_the_target_directory() {
    let root = scratch("install");

    let report = SkillPublisher::from_cli(&build_cli("skills-install"))
        .scope(SkillScope::Project)
        .directory(&root)
        .install()
        .await
        .map(|result| SkillReport::from_result(&result))
        .expect("installing should succeed");

    // The canonical location is what every agent reads, so assert on files
    // rather than on the returned counts alone.
    let canonical = root.join(".agents").join("skills");
    assert!(
        canonical.is_dir(),
        "canonical skills directory should exist"
    );

    let installed: Vec<String> = fs::read_dir(&canonical)
        .expect("skills directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        !installed.is_empty(),
        "at least one skill directory should be installed"
    );
    assert!(report.installed > 0);
    assert_eq!(report.installed, installed.len());

    // Every installed skill must carry the file agents actually load.
    for name in &installed {
        let skill_file = canonical.join(name).join("SKILL.md");
        assert!(skill_file.is_file(), "{name} should contain SKILL.md");
        let body = fs::read_to_string(&skill_file).expect("skill body");
        assert!(
            !body.trim().is_empty(),
            "{name} SKILL.md should have content"
        );
    }

    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn an_installed_skill_describes_the_commands_it_came_from() {
    let root = scratch("content");

    SkillPublisher::from_cli(&build_cli("skills-content"))
        .scope(SkillScope::Project)
        .directory(&root)
        .install()
        .await
        .expect("installing should succeed");

    let canonical = root.join(".agents").join("skills");
    let body: String = fs::read_dir(&canonical)
        .expect("skills directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("SKILL.md"))
        .filter(|path| path.is_file())
        .map(|path| fs::read_to_string(path).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    // The generated prompt artifact must name the real commands, or an agent
    // reading it learns nothing about this application.
    assert!(
        body.contains("add"),
        "skills should mention the add command"
    );
    assert!(
        body.contains("list"),
        "skills should mention the list command"
    );

    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn installing_twice_replaces_rather_than_duplicates() {
    let root = scratch("repeat");
    let publisher = SkillPublisher::from_cli(&build_cli("skills-repeat"))
        .scope(SkillScope::Project)
        .directory(&root);

    let first = publisher
        .install()
        .await
        .map(|result| SkillReport::from_result(&result))
        .expect("first install");
    let second = publisher
        .install()
        .await
        .map(|result| SkillReport::from_result(&result))
        .expect("second install");

    assert_eq!(first.installed, second.installed);
    let count = fs::read_dir(root.join(".agents").join("skills"))
        .expect("skills directory")
        .count();
    assert_eq!(count, second.installed);

    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_cli_with_no_commands_reports_nothing_to_install() {
    let publisher = SkillPublisher::from_cli(&Cli::create("empty"));

    assert!(publisher.is_empty());
}
