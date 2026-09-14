//! Agent skill installation from the desktop application.
//!
//! Shipping a command graph as a window should not cost it its agent surface.
//! A person who installs the application has the same coding agents on their
//! machine as anyone else, and those agents find commands through skill files.
//!
//! This module is a Publisher boundary, not a compiler. It hands the CLI's own
//! Prompt Compiler input to [`incurs::sync_skills::sync`], which generates the
//! `SKILL.md` files and installs them to detected agents. Nothing here decides
//! what a skill says.

use std::path::PathBuf;
use std::sync::Arc;

use incurs::cli::Cli;
use incurs::skill::CommandInfo;
use incurs::sync_skills::{SyncOptions, SyncResult};

/// Where skill files are installed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SkillScope {
    /// Install for the whole account, under the user's home directory.
    ///
    /// This is the default for a shipped application, which has no project
    /// directory to belong to.
    #[default]
    Global,
    /// Install into one project directory.
    Project,
}

/// The command information needed to generate and install skill files.
///
/// Built from a [`Cli`], so the skills an application installs describe the
/// same commands its window runs.
#[derive(Clone)]
pub struct SkillPublisher {
    name: String,
    description: Option<String>,
    commands: Arc<Vec<CommandInfo>>,
    scope: SkillScope,
    directory: Option<PathBuf>,
}

impl SkillPublisher {
    /// Captures one CLI's commands for later installation.
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            name: cli.name.clone(),
            description: cli.description.clone(),
            commands: Arc::new(cli.skill_command_info()),
            scope: SkillScope::default(),
            directory: None,
        }
    }

    /// Sets where skills are installed.
    pub fn scope(mut self, scope: SkillScope) -> Self {
        self.scope = scope;
        self
    }

    /// Sets the project directory used by [`SkillScope::Project`].
    ///
    /// Ignored for a global install, which always uses the home directory.
    pub fn directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }

    /// Returns the application name the skills are generated for.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether there is anything to install.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Generates the skill files and installs them.
    ///
    /// # Errors
    ///
    /// Returns the incurs error when generation or installation fails.
    pub async fn install(&self) -> Result<SyncResult, incurs::errors::Error> {
        incurs::sync_skills::sync(
            &self.name,
            &self.commands,
            &SyncOptions {
                cwd: self
                    .directory
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
                depth: Some(1),
                description: self.description.clone(),
                global: self.scope == SkillScope::Global,
                include: None,
            },
        )
        .await
    }
}

/// A finished installation, described for a person rather than an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillReport {
    /// How many skill files were installed.
    pub installed: usize,
    /// The agent applications that received them, in stable order.
    pub agents: Vec<String>,
    /// Where the canonical copies were written.
    pub paths: Vec<PathBuf>,
}

impl SkillReport {
    /// Summarizes one sync result.
    pub fn from_result(result: &SyncResult) -> Self {
        let mut agents: Vec<String> = result
            .agents
            .iter()
            .map(|install| install.agent.clone())
            .collect();
        agents.sort();
        agents.dedup();

        Self {
            installed: result.skills.len(),
            agents,
            paths: result.paths.clone(),
        }
    }

    /// Renders a sentence describing what happened.
    ///
    /// Names the agents when any were found, because that is what tells a
    /// person the install reached something they use.
    pub fn summary(&self) -> String {
        if self.installed == 0 {
            return "There were no skills to install.".to_string();
        }

        let skills = if self.installed == 1 {
            "1 skill".to_string()
        } else {
            format!("{} skills", self.installed)
        };

        match self.agents.len() {
            0 => format!(
                "Installed {skills}. No agent applications were detected, so they are ready for \
                 any agent that reads the shared skills folder."
            ),
            _ => format!("Installed {skills} for {}.", join_words(&self.agents)),
        }
    }
}

/// Joins names as a person would read them.
fn join_words(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use incurs::agents::AgentInstall;
    use incurs::sync_skills::SyncedSkill;

    fn result(skills: usize, agents: &[&str]) -> SyncResult {
        SyncResult {
            skills: (0..skills)
                .map(|index| SyncedSkill {
                    name: format!("skill-{index}"),
                    description: None,
                    external: false,
                })
                .collect(),
            paths: vec![PathBuf::from("/tmp/.agents/skills/todo")],
            agents: agents
                .iter()
                .map(|agent| AgentInstall {
                    agent: (*agent).to_string(),
                    path: PathBuf::from("/tmp"),
                    mode: incurs::agents::InstallMode::Symlink,
                })
                .collect(),
        }
    }

    #[test]
    fn a_report_names_the_agents_that_received_skills() {
        let report = SkillReport::from_result(&result(3, &["Claude Code", "Cursor"]));

        assert_eq!(report.installed, 3);
        assert_eq!(report.agents, vec!["Claude Code", "Cursor"]);
        assert_eq!(
            report.summary(),
            "Installed 3 skills for Claude Code and Cursor."
        );
    }

    #[test]
    fn a_repeated_agent_is_named_once() {
        let report = SkillReport::from_result(&result(1, &["Cursor", "Cursor"]));

        assert_eq!(report.agents, vec!["Cursor"]);
        assert_eq!(report.summary(), "Installed 1 skill for Cursor.");
    }

    #[test]
    fn three_agents_read_as_a_list() {
        let report = SkillReport::from_result(&result(2, &["Amp", "Cursor", "Zed"]));

        assert_eq!(
            report.summary(),
            "Installed 2 skills for Amp, Cursor, and Zed."
        );
    }

    #[test]
    fn installing_with_no_detected_agent_still_explains_what_happened() {
        let report = SkillReport::from_result(&result(2, &[]));

        let summary = report.summary();
        assert!(summary.contains("Installed 2 skills"));
        assert!(summary.contains("shared skills folder"));
    }

    #[test]
    fn an_empty_install_says_so_rather_than_claiming_success() {
        let report = SkillReport::from_result(&result(0, &[]));

        assert_eq!(report.summary(), "There were no skills to install.");
    }
}
