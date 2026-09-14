//! macOS application bundle publication.
//!
//! A compiled executable is not something a non-technical person can install.
//! This module is a Publisher: it wraps an already-built binary in the
//! `.app` directory layout macOS expects, so the result can be dragged into
//! Applications and opened from the Dock or Finder.
//!
//! It compiles nothing and signs nothing. Distributing a bundle outside your
//! own machine additionally requires code signing and notarization, which are
//! account-bound operations this crate deliberately leaves to the publisher.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Inputs for one macOS application bundle.
#[derive(Debug, Clone)]
pub struct MacBundle {
    /// Display name, used for `Name.app` and the menu bar.
    pub name: String,
    /// Reverse-domain bundle identifier, such as `com.example.todo`.
    pub identifier: String,
    /// Version shown in Finder, such as `1.0.0`.
    pub version: String,
    /// Path to the already-built executable.
    pub executable: PathBuf,
    /// Optional `.icns` icon file.
    pub icon: Option<PathBuf>,
    /// Minimum macOS version the bundle declares.
    pub minimum_system_version: String,
}

impl MacBundle {
    /// Creates bundle inputs with conventional defaults.
    pub fn new(
        name: impl Into<String>,
        identifier: impl Into<String>,
        executable: impl Into<PathBuf>,
    ) -> Self {
        Self {
            name: name.into(),
            identifier: identifier.into(),
            version: "0.1.0".to_string(),
            executable: executable.into(),
            icon: None,
            minimum_system_version: "10.15".to_string(),
        }
    }

    /// Sets the bundle version.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Sets the `.icns` icon copied into the bundle.
    pub fn icon(mut self, icon: impl Into<PathBuf>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Writes the bundle into `output_dir` and returns the `.app` path.
    ///
    /// An existing bundle at that path is replaced, so repeated builds are
    /// idempotent rather than accumulating stale files.
    ///
    /// # Errors
    ///
    /// Returns an error when the executable is missing or any bundle path
    /// cannot be created.
    pub fn write(&self, output_dir: impl AsRef<Path>) -> io::Result<PathBuf> {
        if !self.executable.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("executable not found: {}", self.executable.display()),
            ));
        }

        let app = output_dir.as_ref().join(format!("{}.app", self.name));
        if app.exists() {
            fs::remove_dir_all(&app)?;
        }

        let contents = app.join("Contents");
        let macos = contents.join("MacOS");
        let resources = contents.join("Resources");
        fs::create_dir_all(&macos)?;
        fs::create_dir_all(&resources)?;

        let executable_name = self
            .executable
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.name.clone());

        let destination = macos.join(&executable_name);
        fs::copy(&self.executable, &destination)?;
        make_executable(&destination)?;

        let icon_file = match &self.icon {
            Some(icon) => {
                let file_name = icon.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "icon has no name")
                })?;
                fs::copy(icon, resources.join(file_name))?;
                Some(file_name.to_string_lossy().to_string())
            }
            None => None,
        };

        fs::write(
            contents.join("Info.plist"),
            self.info_plist(&executable_name, icon_file.as_deref()),
        )?;
        fs::write(contents.join("PkgInfo"), "APPL????")?;

        Ok(app)
    }

    /// Renders the bundle's `Info.plist`.
    pub fn info_plist(&self, executable_name: &str, icon_file: Option<&str>) -> String {
        let mut entries = vec![
            ("CFBundleName", self.name.clone()),
            ("CFBundleDisplayName", self.name.clone()),
            ("CFBundleIdentifier", self.identifier.clone()),
            ("CFBundleExecutable", executable_name.to_string()),
            ("CFBundleVersion", self.version.clone()),
            ("CFBundleShortVersionString", self.version.clone()),
            ("CFBundlePackageType", "APPL".to_string()),
            ("CFBundleInfoDictionaryVersion", "6.0".to_string()),
            (
                "LSMinimumSystemVersion",
                self.minimum_system_version.clone(),
            ),
        ];
        if let Some(icon_file) = icon_file {
            entries.push(("CFBundleIconFile", icon_file.to_string()));
        }

        let body: String = entries
            .into_iter()
            .map(|(key, value)| {
                format!(
                    "\t<key>{}</key>\n\t<string>{}</string>\n",
                    escape_xml(key),
                    escape_xml(&value)
                )
            })
            .collect();

        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             {body}\t<key>NSHighResolutionCapable</key>\n\t<true/>\n\
             </dict>\n\
             </plist>\n"
        )
    }
}

/// Marks a path executable for its owner, group, and others.
#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
}

/// Leaves permissions unchanged on platforms without a Unix mode.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Escapes the five XML entities so a plist value cannot break the document.
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a scratch directory holding a stand-in executable.
    fn fixture(label: &str) -> (PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("incurs-app-gpui-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("scratch directory");
        let executable = root.join("todo");
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("stand-in executable");
        (root, executable)
    }

    #[test]
    fn bundle_layout_matches_what_macos_expects() {
        let (root, executable) = fixture("layout");

        let app = MacBundle::new("Todo", "com.example.todo", &executable)
            .version("1.2.3")
            .write(&root)
            .expect("bundle should be written");

        assert_eq!(app, root.join("Todo.app"));
        assert!(app.join("Contents/Info.plist").is_file());
        assert!(app.join("Contents/MacOS/todo").is_file());
        assert!(app.join("Contents/Resources").is_dir());
        assert_eq!(
            fs::read_to_string(app.join("Contents/PkgInfo")).unwrap(),
            "APPL????"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_copied_executable_stays_executable() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let (root, executable) = fixture("mode");
            let app = MacBundle::new("Todo", "com.example.todo", &executable)
                .write(&root)
                .expect("bundle should be written");

            let mode = fs::metadata(app.join("Contents/MacOS/todo"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);

            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn the_plist_declares_the_identity_finder_reads() {
        let (root, executable) = fixture("plist");

        let app = MacBundle::new("Todo", "com.example.todo", &executable)
            .version("1.2.3")
            .write(&root)
            .expect("bundle should be written");
        let plist = fs::read_to_string(app.join("Contents/Info.plist")).unwrap();

        assert!(
            plist.contains("<key>CFBundleIdentifier</key>\n\t<string>com.example.todo</string>")
        );
        assert!(plist.contains("<key>CFBundleExecutable</key>\n\t<string>todo</string>"));
        assert!(plist.contains("<key>CFBundleShortVersionString</key>\n\t<string>1.2.3</string>"));
        assert!(plist.contains("<key>NSHighResolutionCapable</key>\n\t<true/>"));
        assert!(!plist.contains("CFBundleIconFile"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_icon_is_copied_and_declared() {
        let (root, executable) = fixture("icon");
        let icon = root.join("app.icns");
        fs::write(&icon, b"icns").unwrap();

        let app = MacBundle::new("Todo", "com.example.todo", &executable)
            .icon(&icon)
            .write(&root)
            .expect("bundle should be written");

        assert!(app.join("Contents/Resources/app.icns").is_file());
        let plist = fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<key>CFBundleIconFile</key>\n\t<string>app.icns</string>"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn writing_twice_replaces_rather_than_accumulates() {
        let (root, executable) = fixture("replace");
        let bundle = MacBundle::new("Todo", "com.example.todo", &executable);

        let app = bundle.write(&root).expect("first write");
        fs::write(app.join("Contents/MacOS/stale"), b"old").unwrap();
        let app = bundle.write(&root).expect("second write");

        assert!(!app.join("Contents/MacOS/stale").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_executable_is_reported_rather_than_producing_a_broken_bundle() {
        let (root, _) = fixture("missing");

        let error = MacBundle::new("Todo", "com.example.todo", root.join("absent"))
            .write(&root)
            .expect_err("a missing executable must fail");

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!root.join("Todo.app").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn plist_values_are_xml_escaped() {
        let bundle = MacBundle::new("Bits & Bobs", "com.example.<todo>", "todo");

        let plist = bundle.info_plist("todo", None);

        assert!(plist.contains("<string>Bits &amp; Bobs</string>"));
        assert!(plist.contains("<string>com.example.&lt;todo&gt;</string>"));
    }
}
