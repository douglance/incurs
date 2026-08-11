use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const INSTALL_SCHEMA: &str = "incurs.plugin-install.v1";
const INSTALLER_EXTENSION: &str = "io.github.douglance.incurs";
static NEXT_STAGE: AtomicUsize = AtomicUsize::new(0);

/// Options for installing one local Agent Plugin directory.
pub(crate) struct InstallOptions {
    /// Source Agent Plugin directory.
    pub(crate) source: PathBuf,
    /// Platform user-data root.
    pub(crate) data_home: PathBuf,
    /// Directory that receives a bundled command.
    pub(crate) bin_dir: PathBuf,
    /// Whether to replace the same managed installation.
    pub(crate) force: bool,
}

/// Structured result of installing one Agent Plugin.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallResult {
    /// Installed plugin name.
    pub(crate) name: String,
    /// Installed plugin version, when declared.
    pub(crate) version: Option<String>,
    /// Managed package directory.
    pub(crate) plugin_root: PathBuf,
    /// Persistent plugin-data directory.
    pub(crate) data_root: PathBuf,
    /// Installed command path, when the package contains a Tool Runtime.
    pub(crate) command: Option<PathBuf>,
    /// Whether the command directory is already on `PATH`.
    pub(crate) path_ready: bool,
    /// Actionable `PATH` warning, when needed.
    pub(crate) warning: Option<String>,
}

/// Options for removing one managed Agent Plugin.
pub(crate) struct UninstallOptions {
    /// Installed plugin name.
    pub(crate) name: String,
    /// Platform user-data root.
    pub(crate) data_home: PathBuf,
    /// Whether to remove persistent plugin data.
    pub(crate) purge: bool,
}

/// Structured result of removing one Agent Plugin.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UninstallResult {
    /// Removed plugin name.
    pub(crate) name: String,
    /// Former managed package directory.
    pub(crate) plugin_root: PathBuf,
    /// Persistent plugin-data directory.
    pub(crate) data_root: PathBuf,
    /// Removed command path, when the package contained a Tool Runtime.
    pub(crate) command: Option<PathBuf>,
    /// Whether persistent plugin data was removed.
    pub(crate) purged_data: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallRecord {
    schema: String,
    name: String,
    version: Option<String>,
    plugin_root: PathBuf,
    data_root: PathBuf,
    command: Option<InstalledCommand>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstalledCommand {
    name: String,
    path: PathBuf,
    digest: String,
}

struct ToolRuntime {
    path: PathBuf,
    shell_command: String,
}

/// Resolve the default user-data root for managed plugin state.
pub(crate) fn default_data_home() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("INCURS_DATA_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    dirs::data_local_dir().ok_or_else(|| "cannot resolve a user data directory".to_string())
}

/// Resolve the default directory for managed plugin commands.
pub(crate) fn default_bin_dir() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("INCURS_BIN_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = dirs::executable_dir() {
        return Ok(path);
    }
    if cfg!(target_os = "macos") {
        return dirs::home_dir()
            .map(|home| home.join(".local/bin"))
            .ok_or_else(|| "cannot resolve the user home directory".to_string());
    }
    if cfg!(windows) {
        return dirs::data_local_dir()
            .map(|data| data.join("incurs/bin"))
            .ok_or_else(|| "cannot resolve a user command directory".to_string());
    }
    Err("cannot resolve a user command directory; pass --bin-dir".to_string())
}

/// Validate and install one local Agent Plugin directory.
pub(crate) fn install(options: InstallOptions) -> Result<InstallResult, String> {
    let source = options
        .source
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", options.source.display()))?;
    let base = options.data_home.join("incurs");
    let provisional_data = base.join("plugin-data/pending");
    let report = incurs::agent_plugin::loader::load_agent_plugin(
        &source,
        &incurs::agent_plugin::loader::AgentPluginLoadOptions {
            plugin_data_root: provisional_data,
            ..Default::default()
        },
    );
    let errors = report
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.severity
                == incurs::agent_plugin::loader::AgentPluginDiagnosticSeverity::Error
        })
        .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(format!(
            "Agent Plugin validation failed: {}",
            errors.join("; ")
        ));
    }
    let plugin = report
        .plugin
        .ok_or_else(|| "Agent Plugin manifest is invalid".to_string())?;
    let runtime = parse_tool_runtime(&plugin)?;
    let name = plugin.manifest.name.clone();
    let version = plugin.manifest.version.clone();
    let plugins = base.join("plugins");
    let installs = base.join("installs");
    let plugin_root = plugins.join(&name);
    let data_root = base.join("plugin-data").join(&name);
    let record_path = installs.join(format!("{name}.json"));
    let previous = read_optional_record(&record_path)?;

    if plugin_root.exists() || record_path.exists() {
        if !options.force {
            return Err(format!(
                "plugin {name} is already installed; pass --force to replace it"
            ));
        }
        let previous = previous.as_ref().ok_or_else(|| {
            format!(
                "plugin path exists without a managed install record: {}",
                plugin_root.display()
            )
        })?;
        validate_record(previous, &name, &plugin_root, &data_root, &record_path)?;
    }

    fs::create_dir_all(&plugins).map_err(|error| error.to_string())?;
    fs::create_dir_all(&installs).map_err(|error| error.to_string())?;
    fs::create_dir_all(&options.bin_dir).map_err(|error| error.to_string())?;
    let bin_dir = options
        .bin_dir
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", options.bin_dir.display()))?;
    let command = runtime
        .as_ref()
        .map(|runtime| bin_dir.join(command_file_name(&runtime.shell_command)));
    if let Some(command) = &command
        && command.exists()
    {
        let owned = previous
            .as_ref()
            .and_then(|record| record.command.as_ref())
            .is_some_and(|installed| installed.path == *command && record_path.exists());
        if !options.force || !owned {
            return Err(format!(
                "command path already exists and is not replaceable: {}",
                command.display()
            ));
        }
    }

    let nonce = format!(
        "{}-{}",
        std::process::id(),
        NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
    );
    let staged_plugin = plugins.join(format!(".{name}.stage-{nonce}"));
    copy_plugin_directory(&source, &staged_plugin)?;

    let staged_command = command.as_ref().map(|command| {
        let file = command
            .file_name()
            .expect("validated command has a file name");
        command.with_file_name(format!(".{}.stage-{nonce}", file.to_string_lossy()))
    });
    let installed_command = match (&runtime, &command, &staged_command) {
        (Some(runtime), Some(command), Some(staged_command)) => {
            let source = staged_plugin.join(&runtime.path);
            copy_file(&source, staged_command)?;
            Some(InstalledCommand {
                name: runtime.shell_command.clone(),
                path: command.clone(),
                digest: file_digest(staged_command)?,
            })
        }
        _ => None,
    };
    let record = InstallRecord {
        schema: INSTALL_SCHEMA.to_string(),
        name: name.clone(),
        version: version.clone(),
        plugin_root: plugin_root.clone(),
        data_root: data_root.clone(),
        command: installed_command,
    };
    let staged_record = installs.join(format!(".{name}.stage-{nonce}.json"));
    write_record(&staged_record, &record)?;

    if let Err(error) = commit_install(
        &plugin_root,
        &staged_plugin,
        command.as_deref(),
        staged_command.as_deref(),
        &record_path,
        &staged_record,
        &nonce,
    ) {
        let _ = remove_path(&staged_plugin);
        if let Some(path) = &staged_command {
            let _ = remove_path(path);
        }
        let _ = remove_path(&staged_record);
        return Err(error);
    }

    let path_ready = path_contains(&bin_dir);
    let warning = runtime.as_ref().and_then(|runtime| {
        (!path_ready).then(|| {
            format!(
                "{} is not on PATH; add it before invoking {}",
                bin_dir.display(),
                runtime.shell_command
            )
        })
    });
    Ok(InstallResult {
        name,
        version,
        plugin_root,
        data_root,
        command,
        path_ready,
        warning,
    })
}

/// Remove one managed Agent Plugin and optionally purge its persistent data.
pub(crate) fn uninstall(options: UninstallOptions) -> Result<UninstallResult, String> {
    if !is_shell_command(&options.name) {
        return Err(format!("invalid plugin name: {}", options.name));
    }
    let base = options.data_home.join("incurs");
    let record_path = base.join("installs").join(format!("{}.json", options.name));
    let record = read_record(&record_path)?;
    let plugin_root = base.join("plugins").join(&options.name);
    let data_root = base.join("plugin-data").join(&options.name);
    validate_record(
        &record,
        &options.name,
        &plugin_root,
        &data_root,
        &record_path,
    )?;
    if let Some(command) = &record.command
        && command.path.exists()
        && file_digest(&command.path)? != command.digest
    {
        return Err(format!(
            "installed command was modified and will not be removed: {}",
            command.path.display()
        ));
    }

    if let Some(command) = &record.command {
        remove_path(&command.path)?;
    }
    remove_path(&record.plugin_root)?;
    remove_path(&record_path)?;
    let mut purged_data = false;
    if options.purge && record.data_root.exists() {
        remove_path(&record.data_root)?;
        purged_data = true;
    }
    Ok(UninstallResult {
        name: record.name,
        plugin_root: record.plugin_root,
        data_root: record.data_root,
        command: record.command.map(|command| command.path),
        purged_data,
    })
}

fn validate_record(
    record: &InstallRecord,
    name: &str,
    plugin_root: &Path,
    data_root: &Path,
    record_path: &Path,
) -> Result<(), String> {
    if record.schema != INSTALL_SCHEMA
        || record.name != name
        || record.plugin_root != plugin_root
        || record.data_root != data_root
        || record.command.as_ref().is_some_and(|command| {
            let expected = command_file_name(&command.name);
            !is_shell_command(&command.name)
                || command.path.file_name().and_then(|file| file.to_str())
                    != Some(expected.as_str())
        })
    {
        return Err(format!("invalid install record: {}", record_path.display()));
    }
    Ok(())
}

fn parse_tool_runtime(
    plugin: &incurs::agent_plugin::loader::LoadedAgentPlugin,
) -> Result<Option<ToolRuntime>, String> {
    let Some(extension) = plugin.extensions.get(INSTALLER_EXTENSION) else {
        return Ok(None);
    };
    let Some(value) = extension.get("toolRuntime") else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| "installer toolRuntime metadata must be an object".to_string())?;
    let string = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("installer toolRuntime.{name} must be a string"))
    };
    let os = string("os")?;
    let arch = string("arch")?;
    if os != env::consts::OS || arch != env::consts::ARCH {
        return Err(format!(
            "plugin Tool Runtime targets {os}/{arch}, but this host is {}/{}",
            env::consts::OS,
            env::consts::ARCH
        ));
    }
    let shell_command = string("shellCommand")?.to_string();
    if !is_shell_command(&shell_command) || shell_command.ends_with(".exe") {
        return Err("installer shellCommand must be one bare token without .exe".to_string());
    }
    let configured = string("path")?;
    if !configured.starts_with("./") {
        return Err("installer toolRuntime.path must begin with ./".to_string());
    }
    let path = Path::new(configured);
    if path
        .components()
        .any(|component| !matches!(component, Component::CurDir | Component::Normal(_)))
    {
        return Err("installer toolRuntime.path must remain inside the plugin".to_string());
    }
    let relative = path
        .strip_prefix(".")
        .map_err(|_| "installer toolRuntime.path must begin with ./".to_string())?;
    let expected = PathBuf::from("bin").join(command_file_name(&shell_command));
    if relative != expected {
        return Err(format!(
            "installer toolRuntime.path must be ./{}",
            expected.display()
        ));
    }
    let source = plugin.root.join(relative);
    let metadata = fs::symlink_metadata(&source)
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("installer Tool Runtime must be a regular file and not a symlink".to_string());
    }
    let resolved = source
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", source.display()))?;
    if !resolved.starts_with(&plugin.root) {
        return Err("installer Tool Runtime resolves outside the plugin".to_string());
    }
    Ok(Some(ToolRuntime {
        path: relative.to_path_buf(),
        shell_command,
    }))
}

fn command_file_name(command: &str) -> String {
    if cfg!(windows) {
        format!("{command}.exe")
    } else {
        command.to_string()
    }
}

fn is_shell_command(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.chars().any(char::is_whitespace)
        && !value.contains('/')
        && !value.contains('\\')
}

fn copy_plugin_directory(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "plugin source must be a directory and not a symlink: {}",
            source.display()
        ));
    }
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source = entry.path();
        let destination = destination.join(entry.file_name());
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "symlinks are not installed from Agent Plugin packages: {}",
                source.display()
            ));
        }
        if kind.is_dir() {
            copy_plugin_directory(&source, &destination)?;
        } else if kind.is_file() {
            copy_file(&source, &destination)?;
        } else {
            return Err(format!(
                "unsupported filesystem entry in Agent Plugin: {}",
                source.display()
            ));
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|error| format!("cannot copy {}: {error}", destination.display()))
}

fn commit_install(
    plugin: &Path,
    staged_plugin: &Path,
    command: Option<&Path>,
    staged_command: Option<&Path>,
    record: &Path,
    staged_record: &Path,
    nonce: &str,
) -> Result<(), String> {
    let plugin_backup = plugin.with_file_name(format!(
        ".{}.backup-{nonce}",
        plugin.file_name().unwrap_or_default().to_string_lossy()
    ));
    let command_backup = command.map(|path| {
        path.with_file_name(format!(
            ".{}.backup-{nonce}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    });
    let record_backup = record.with_file_name(format!(
        ".{}.backup-{nonce}",
        record.file_name().unwrap_or_default().to_string_lossy()
    ));

    let result = (|| -> Result<(), String> {
        if plugin.exists() {
            fs::rename(plugin, &plugin_backup).map_err(|error| error.to_string())?;
        }
        if let (Some(command), Some(backup)) = (command, command_backup.as_deref())
            && command.exists()
        {
            fs::rename(command, backup).map_err(|error| error.to_string())?;
        }
        if record.exists() {
            fs::rename(record, &record_backup).map_err(|error| error.to_string())?;
        }
        fs::rename(staged_plugin, plugin).map_err(|error| error.to_string())?;
        if let (Some(command), Some(staged_command)) = (command, staged_command) {
            fs::rename(staged_command, command).map_err(|error| error.to_string())?;
        }
        fs::rename(staged_record, record).map_err(|error| error.to_string())?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = remove_path(plugin);
        if plugin_backup.exists() {
            let _ = fs::rename(&plugin_backup, plugin);
        }
        if let Some(command) = command {
            let _ = remove_path(command);
        }
        if let Some(backup) = command_backup.as_deref()
            && backup.exists()
            && let Some(command) = command
        {
            let _ = fs::rename(backup, command);
        }
        let _ = remove_path(record);
        if record_backup.exists() {
            let _ = fs::rename(&record_backup, record);
        }
        return Err(format!("cannot commit plugin installation: {error}"));
    }

    let _ = remove_path(&plugin_backup);
    if let Some(backup) = command_backup.as_deref() {
        let _ = remove_path(backup);
    }
    let _ = remove_path(&record_backup);
    Ok(())
}

fn write_record(path: &Path, record: &InstallRecord) -> Result<(), String> {
    let contents = serde_json::to_string_pretty(record).map_err(|error| error.to_string())? + "\n";
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn read_optional_record(path: &Path) -> Result<Option<InstallRecord>, String> {
    if !path.exists() {
        return Ok(None);
    }
    read_record(path).map(Some)
}

fn read_record(path: &Path) -> Result<InstallRecord, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("invalid install record {}: {error}", path.display()))
}

fn file_digest(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn path_contains(dir: &Path) -> bool {
    env::var_os("PATH")
        .map(|path| {
            env::split_paths(&path).any(|entry| {
                entry
                    .canonicalize()
                    .map(|entry| entry == dir)
                    .unwrap_or(entry == dir)
            })
        })
        .unwrap_or(false)
}

fn remove_path(path: &Path) -> Result<(), String> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .map_err(|error| format!("cannot remove {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "incurs-plugin-install-{}-{name}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    fn source_plugin(root: &Path) {
        let executable = if cfg!(windows) {
            "demo-tools.exe"
        } else {
            "demo-tools"
        };
        fs::create_dir_all(root.join("skills/demo-tools")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(
            root.join("skills/demo-tools/SKILL.md"),
            "---\nname: demo-tools\ndescription: Run demo tools.\n---\n\nUse the tools.\n",
        )
        .unwrap();
        fs::copy(
            std::env::current_exe().unwrap(),
            root.join("bin").join(executable),
        )
        .unwrap();
        fs::write(
            root.join("plugin.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "name": "demo-tools",
                "version": "1.2.3",
                "extensions": {
                    "io.github.douglance.incurs": {
                        "toolRuntime": {
                            "arch": std::env::consts::ARCH,
                            "os": std::env::consts::OS,
                            "path": format!("./bin/{executable}"),
                            "shellCommand": "demo-tools"
                        }
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("mcp.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
                "mcpServers": {
                    "demo-tools": {
                        "type": "stdio",
                        "command": format!("./bin/{executable}"),
                        "args": ["--mcp"]
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn install_and_uninstall_manage_plugin_command_and_persistent_data() {
        let root = temp_root("lifecycle");
        let source = root.join("source");
        let data_home = root.join("data-home");
        let bin_dir = root.join("bin-home");
        source_plugin(&source);

        let installed = install(InstallOptions {
            source,
            data_home: data_home.clone(),
            bin_dir,
            force: false,
        })
        .unwrap();

        assert!(installed.plugin_root.join("plugin.json").is_file());
        assert!(installed.command.as_ref().unwrap().is_file());
        fs::create_dir_all(&installed.data_root).unwrap();
        fs::write(installed.data_root.join("keep.txt"), "keep").unwrap();

        let removed = uninstall(UninstallOptions {
            name: "demo-tools".to_string(),
            data_home,
            purge: false,
        })
        .unwrap();

        assert!(!removed.plugin_root.exists());
        assert!(!removed.command.unwrap().exists());
        assert_eq!(
            fs::read_to_string(removed.data_root.join("keep.txt")).unwrap(),
            "keep"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn install_rejects_a_runtime_for_another_platform() {
        let root = temp_root("platform");
        let source = root.join("source");
        source_plugin(&source);
        let manifest_path = source.join("plugin.json");
        let mut manifest: Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        manifest["extensions"][INSTALLER_EXTENSION]["toolRuntime"]["arch"] =
            Value::String("other-arch".to_string());
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let error = install(InstallOptions {
            source,
            data_home: root.join("data-home"),
            bin_dir: root.join("bin-home"),
            force: false,
        })
        .unwrap_err();

        assert!(error.contains("but this host is"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn install_never_replaces_an_unowned_command() {
        let root = temp_root("command-conflict");
        let source = root.join("source");
        let bin_dir = root.join("bin-home");
        source_plugin(&source);
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join(command_file_name("demo-tools")), "unowned").unwrap();

        let error = install(InstallOptions {
            source,
            data_home: root.join("data-home"),
            bin_dir,
            force: true,
        })
        .unwrap_err();

        assert!(error.contains("not replaceable"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn force_never_replaces_an_unmanaged_plugin_directory() {
        let root = temp_root("plugin-conflict");
        let source = root.join("source");
        let data_home = root.join("data-home");
        let existing = data_home.join("incurs/plugins/demo-tools");
        source_plugin(&source);
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("keep.txt"), "unmanaged").unwrap();

        let error = install(InstallOptions {
            source,
            data_home,
            bin_dir: root.join("bin-home"),
            force: true,
        })
        .unwrap_err();

        assert!(error.contains("without a managed install record"));
        assert_eq!(
            fs::read_to_string(existing.join("keep.txt")).unwrap(),
            "unmanaged"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn install_rejects_symlinks_anywhere_in_the_package() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let source = root.join("source");
        source_plugin(&source);
        fs::write(root.join("outside"), "outside").unwrap();
        symlink(root.join("outside"), source.join("linked")).unwrap();

        let error = install(InstallOptions {
            source,
            data_home: root.join("data-home"),
            bin_dir: root.join("bin-home"),
            force: false,
        })
        .unwrap_err();

        assert!(error.contains("symlinks are not installed"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn uninstall_refuses_to_remove_a_modified_command() {
        let root = temp_root("modified-command");
        let source = root.join("source");
        let data_home = root.join("data-home");
        source_plugin(&source);
        let installed = install(InstallOptions {
            source,
            data_home: data_home.clone(),
            bin_dir: root.join("bin-home"),
            force: false,
        })
        .unwrap();
        fs::write(installed.command.as_ref().unwrap(), "modified").unwrap();

        let error = uninstall(UninstallOptions {
            name: "demo-tools".to_string(),
            data_home,
            purge: true,
        })
        .unwrap_err();

        assert!(error.contains("was modified"));
        assert!(installed.plugin_root.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn uninstall_rejects_paths_forged_in_the_install_record() {
        let root = temp_root("forged-record");
        let source = root.join("source");
        let data_home = root.join("data-home");
        source_plugin(&source);
        install(InstallOptions {
            source,
            data_home: data_home.clone(),
            bin_dir: root.join("bin-home"),
            force: false,
        })
        .unwrap();
        let protected = root.join("protected");
        fs::create_dir_all(&protected).unwrap();
        fs::write(protected.join("keep.txt"), "keep").unwrap();
        let record_path = data_home.join("incurs/installs/demo-tools.json");
        let mut record: Value =
            serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
        record["pluginRoot"] = Value::String(protected.display().to_string());
        fs::write(&record_path, serde_json::to_string_pretty(&record).unwrap()).unwrap();

        let error = uninstall(UninstallOptions {
            name: "demo-tools".to_string(),
            data_home,
            purge: false,
        })
        .unwrap_err();

        assert!(error.contains("invalid install record"));
        assert_eq!(
            fs::read_to_string(protected.join("keep.txt")).unwrap(),
            "keep"
        );
        let _ = fs::remove_dir_all(root);
    }
}
