//! Safe, explicit import of legacy `config/<Group>/ObjectData.xml` files.
//!
//! Migration is persistence-only: it reads legacy XML, applies an explicit path
//! policy, and atomically publishes a new versioned tree. It never removes or
//! overwrites the legacy tree.

use std::{
    fs::{self, OpenOptions},
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::domain::Category;

pub const DESTINATION_VERSION: &str = "v1";
pub const DESTINATION_GROUPS_DIRECTORY: &str = "groups";
pub const DESTINATION_FILE: &str = "ObjectData.xml";

/// Selects where the versioned persistence tree is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataRootPolicy {
    /// Store data below the supplied application directory.
    Portable(PathBuf),
    /// Store data below the current user's writable application-data directory.
    Installed { application_name: String },
}

/// A completed migration and the backup that can be used for recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub backup_path: PathBuf,
    pub destination_root: PathBuf,
    pub recovered: bool,
}

/// Controls how legacy relative values are represented in the imported data.
/// Portable imports retain relative values so the application directory can be
/// moved as a unit. Installed imports repair both relative and executable-
/// relative values against the legacy application root before writing them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRepairPolicy {
    Portable,
    Installed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationItem {
    pub source_directory: PathBuf,
    pub source_file: PathBuf,
    pub destination_directory: PathBuf,
    pub destination_file: PathBuf,
    pub group_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyMigrationPlan {
    pub legacy_root: PathBuf,
    pub destination_root: PathBuf,
    pub path_policy: PathRepairPolicy,
    pub items: Vec<MigrationItem>,
}

#[derive(Debug)]
pub enum MigrationError {
    Io { path: PathBuf, source: io::Error },
    InvalidSource { path: PathBuf, reason: String },
    InvalidCategory { path: PathBuf, reason: String },
    Collision { path: PathBuf },
    Backup { path: PathBuf, source: io::Error },
    Recovery { path: PathBuf, source: io::Error },
    InvalidDataRoot { reason: String },
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::InvalidSource { path, reason } => {
                write!(
                    formatter,
                    "invalid legacy source {}: {reason}",
                    path.display()
                )
            }
            Self::InvalidCategory { path, reason } => {
                write!(formatter, "invalid category {}: {reason}", path.display())
            }
            Self::Collision { path } => write!(
                formatter,
                "migration destination already exists: {}",
                path.display()
            ),
            Self::Backup { path, source } => {
                write!(
                    formatter,
                    "cannot create migration backup {}: {source}",
                    path.display()
                )
            }
            Self::Recovery { path, source } => {
                write!(
                    formatter,
                    "migration recovery failed at {}: {source}",
                    path.display()
                )
            }
            Self::InvalidDataRoot { reason } => {
                write!(formatter, "invalid persistence data root: {reason}")
            }
        }
    }
}

impl std::error::Error for MigrationError {}

/// Resolve a persistence root without ever selecting the installation directory
/// for installed mode. Portable mode is intentionally explicit and unchanged.
pub fn resolve_data_root(policy: DataRootPolicy) -> Result<PathBuf, MigrationError> {
    match policy {
        DataRootPolicy::Portable(root) if root.as_os_str().is_empty() => {
            Err(MigrationError::InvalidDataRoot {
                reason: "portable root is empty".into(),
            })
        }
        DataRootPolicy::Portable(root) => Ok(root),
        DataRootPolicy::Installed { application_name } => {
            let name = stored_group_name(&application_name).map_err(|reason| {
                MigrationError::InvalidDataRoot {
                    reason: format!("invalid application name: {reason}"),
                }
            })?;
            let base = std::env::var_os("LOCALAPPDATA")
                .or_else(|| std::env::var_os("APPDATA"))
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|home| PathBuf::from(home).join(".local/share").into_os_string())
                })
                .ok_or_else(|| MigrationError::InvalidDataRoot {
                    reason: "no user application-data directory is available".into(),
                })?;
            Ok(PathBuf::from(base).join(name))
        }
    }
}

impl LegacyMigrationPlan {
    /// Discover using an explicit portable or installed data-root policy.
    pub fn discover_for_root_policy(
        legacy_root: impl Into<PathBuf>,
        root_policy: DataRootPolicy,
        path_policy: PathRepairPolicy,
    ) -> Result<Self, MigrationError> {
        let destination = resolve_data_root(root_policy)?;
        Self::discover_with_policy(legacy_root, destination, path_policy)
    }

    /// Discover a migration using installed-style path repair.
    pub fn discover(
        legacy_root: impl Into<PathBuf>,
        destination_root: impl Into<PathBuf>,
    ) -> Result<Self, MigrationError> {
        Self::discover_with_policy(legacy_root, destination_root, PathRepairPolicy::Installed)
    }

    /// Discover a plan without creating or changing any files.
    pub fn discover_with_policy(
        legacy_root: impl Into<PathBuf>,
        destination_root: impl Into<PathBuf>,
        path_policy: PathRepairPolicy,
    ) -> Result<Self, MigrationError> {
        let legacy_root = legacy_root.into();
        let destination_root = destination_root.into();
        let config = legacy_root.join("config");
        let entries = fs::read_dir(&config).map_err(|source| MigrationError::Io {
            path: config.clone(),
            source,
        })?;
        let canonical_config = fs::canonicalize(&config).map_err(|source| MigrationError::Io {
            path: config.clone(),
            source,
        })?;
        let mut items = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|source| MigrationError::Io {
                path: config.clone(),
                source,
            })?;
            let source_directory = entry.path();
            let metadata =
                fs::symlink_metadata(&source_directory).map_err(|source| MigrationError::Io {
                    path: source_directory.clone(),
                    source,
                })?;
            if !metadata.is_dir() {
                continue;
            }
            let canonical_directory =
                fs::canonicalize(&source_directory).map_err(|source| MigrationError::Io {
                    path: source_directory.clone(),
                    source,
                })?;
            if !canonical_directory.starts_with(&canonical_config)
                || canonical_directory.parent() != Some(canonical_config.as_path())
            {
                return Err(MigrationError::InvalidSource {
                    path: source_directory,
                    reason: "group directory escapes the legacy config directory".into(),
                });
            }

            let source_file = source_directory.join("ObjectData.xml");
            if !source_file.is_file() {
                continue;
            }
            let canonical_file =
                fs::canonicalize(&source_file).map_err(|source| MigrationError::Io {
                    path: source_file.clone(),
                    source,
                })?;
            if !canonical_file.starts_with(&canonical_directory)
                || canonical_file.parent() != Some(canonical_directory.as_path())
            {
                return Err(MigrationError::InvalidSource {
                    path: source_file,
                    reason: "ObjectData.xml escapes the legacy group directory".into(),
                });
            }
            let category = read_category(&source_file)?;
            let category = repair_paths(category, &legacy_root, path_policy);
            category
                .validate()
                .map_err(|error| MigrationError::InvalidCategory {
                    path: source_file.clone(),
                    reason: format!("validation error: {error}"),
                })?;
            let stored_name = stored_group_name(&category.name).map_err(|reason| {
                MigrationError::InvalidCategory {
                    path: source_file.clone(),
                    reason,
                }
            })?;
            let destination_directory = destination_root
                .join(DESTINATION_VERSION)
                .join(DESTINATION_GROUPS_DIRECTORY)
                .join(&stored_name);
            let destination_file = destination_directory.join(DESTINATION_FILE);
            if destination_file.exists()
                || items
                    .iter()
                    .any(|item: &MigrationItem| item.destination_file == destination_file)
            {
                return Err(MigrationError::Collision {
                    path: destination_file,
                });
            }
            items.push(MigrationItem {
                source_directory,
                source_file,
                destination_directory,
                destination_file,
                group_name: category.name,
            });
        }
        items.sort_by(|left, right| left.destination_file.cmp(&right.destination_file));
        Ok(Self {
            legacy_root,
            destination_root,
            path_policy,
            items,
        })
    }

    /// Stage every output first, then publish the complete version directory in
    /// one rename. A source/read/write failure therefore leaves no destination
    /// files or directories behind.
    pub fn execute(&self) -> Result<(), MigrationError> {
        self.execute_with_backup().map(|_| ())
    }

    fn execute_without_backup(&self) -> Result<(), MigrationError> {
        let version_directory = self.destination_root.join(DESTINATION_VERSION);
        if version_directory.exists() {
            return Err(MigrationError::Collision {
                path: version_directory,
            });
        }
        let staging = self.staging_directory();
        let result = self.stage(&staging).and_then(|_| {
            fs::create_dir_all(&self.destination_root).map_err(|source| MigrationError::Io {
                path: self.destination_root.clone(),
                source,
            })?;
            fs::rename(&staging, &version_directory).map_err(|source| MigrationError::Io {
                path: version_directory.clone(),
                source,
            })
        });
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    /// Execute after taking a copy of the legacy source. The source is never
    /// removed; the returned path is suitable for diagnostics or restoration.
    pub fn execute_with_backup(&self) -> Result<MigrationReport, MigrationError> {
        let backup_path = self.backup_path();
        copy_tree(&self.legacy_root.join("config"), &backup_path).map_err(|source| {
            MigrationError::Backup {
                path: backup_path.clone(),
                source,
            }
        })?;
        if let Err(error) = self.execute_without_backup() {
            return Err(error);
        }
        Ok(MigrationReport {
            backup_path,
            destination_root: self.destination_root.clone(),
            recovered: false,
        })
    }

    /// Restore the backed-up legacy config tree, replacing only that tree.
    pub fn restore_backup(&self, backup_path: &Path) -> Result<(), MigrationError> {
        let config = self.legacy_root.join("config");
        let temporary = self.legacy_root.join(".migration-restore");
        copy_tree(backup_path, &temporary).map_err(|source| MigrationError::Recovery {
            path: temporary.clone(),
            source,
        })?;
        let result = (|| {
            if config.exists() {
                fs::remove_dir_all(&config)?;
            }
            fs::rename(&temporary, &config)
        })();
        if let Err(source) = result {
            let _ = fs::remove_dir_all(&temporary);
            return Err(MigrationError::Recovery {
                path: config,
                source,
            });
        }
        Ok(())
    }

    fn backup_path(&self) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        self.destination_root
            .with_file_name(format!(".migration-backup-{nonce}"))
    }

    fn staging_directory(&self) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        self.destination_root
            .with_file_name(format!(".migration-{nonce}"))
    }

    fn stage(&self, staging: &Path) -> Result<(), MigrationError> {
        for item in &self.items {
            let category = read_category(&item.source_file)?;
            let category = repair_paths(category, &self.legacy_root, self.path_policy);
            category
                .validate()
                .map_err(|error| MigrationError::InvalidCategory {
                    path: item.source_file.clone(),
                    reason: format!("validation error: {error}"),
                })?;
            let relative = item
                .destination_file
                .strip_prefix(self.destination_root.join(DESTINATION_VERSION))
                .expect("discovered destination must be under the version directory");
            let file = staging.join(relative);
            let directory = file.parent().expect("destination file has a parent");
            let output = format!(
                "<!-- taskbar-groups migration format {DESTINATION_VERSION} -->\n{}\n",
                category.to_legacy_xml()
            );
            write_new(directory, &file, output.as_bytes())?;
        }
        Ok(())
    }
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
    } else {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "backup source is not a regular file or directory",
        ));
    }
    Ok(())
}

fn read_category(path: &Path) -> Result<Category, MigrationError> {
    let xml = fs::read_to_string(path).map_err(|source| MigrationError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Category::from_legacy_xml(&xml).map_err(|error| MigrationError::InvalidCategory {
        path: path.to_path_buf(),
        reason: format!("XML parse error: {error:?}"),
    })
}

fn repair_paths(mut category: Category, legacy_root: &Path, policy: PathRepairPolicy) -> Category {
    if policy == PathRepairPolicy::Installed {
        for shortcut in &mut category.shortcut_list {
            if !shortcut.is_windows_app {
                shortcut.file_path = repair_path(&shortcut.file_path, legacy_root);
            }
            shortcut.working_directory = repair_path(&shortcut.working_directory, legacy_root);
        }
    }
    category
}

fn repair_path(value: &str, legacy_root: &Path) -> String {
    if value.trim().is_empty() || is_absolute_legacy_path(value) {
        return value.to_owned();
    }
    legacy_root.join(value).to_string_lossy().into_owned()
}

fn is_absolute_legacy_path(value: &str) -> bool {
    Path::new(value).is_absolute()
        || value.starts_with("\\\\")
        || value.as_bytes().get(1) == Some(&b':')
        || value.contains(':')
}

fn write_new(directory: &Path, file: &Path, contents: &[u8]) -> Result<(), MigrationError> {
    fs::create_dir_all(directory).map_err(|source| MigrationError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
        .map_err(|source| {
            if source.kind() == ErrorKind::AlreadyExists {
                MigrationError::Collision {
                    path: file.to_path_buf(),
                }
            } else {
                MigrationError::Io {
                    path: file.to_path_buf(),
                    source,
                }
            }
        })?;
    if let Err(source) = output.write_all(contents).and_then(|_| output.flush()) {
        drop(output);
        let _ = fs::remove_file(file);
        return Err(MigrationError::Io {
            path: file.to_path_buf(),
            source,
        });
    }
    Ok(())
}

fn stored_group_name(name: &str) -> Result<String, String> {
    let stored = name
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if stored.is_empty()
        || stored == "."
        || stored == ".."
        || stored.contains('/')
        || stored.contains('\\')
        || stored.contains('\0')
    {
        return Err("group name must be a non-empty path component".into());
    }
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "taskbar-groups-migration-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const XML: &str = r#"<Category><Name>Games</Name><ColorString>#123456</ColorString><allowOpenAll>true</allowOpenAll><ShortcutList><ProgramShortcut><FilePath>relative/game.exe</FilePath><isWindowsApp>false</isWindowsApp><name>Game</name><Arguments>--profile &quot;test&quot;</Arguments><WorkingDirectory>relative</WorkingDirectory></ProgramShortcut><ProgramShortcut><FilePath>shell:AppsFolder\Foo!Bar</FilePath><isWindowsApp>true</isWindowsApp><name>Store app</name><Arguments></Arguments><WorkingDirectory></WorkingDirectory></ProgramShortcut></ShortcutList><Width>4</Width><Opacity>30</Opacity></Category>"#;

    fn write_group(root: &Path, directory: &str, xml: &str) {
        let source = root.join("config").join(directory);
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("ObjectData.xml"), xml).unwrap();
    }

    #[test]
    fn imports_multiple_groups_in_stable_order_and_repairs_installed_paths() {
        let root = TempDir::new();
        write_group(&root.0, "Zed", XML.replace("Games", "Zed").as_str());
        write_group(&root.0, "Alpha", XML.replace("Games", "Alpha").as_str());
        let destination = root.0.join("out");
        let plan = LegacyMigrationPlan::discover(&root.0, &destination).unwrap();
        assert_eq!(
            plan.items
                .iter()
                .map(|i| i.group_name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "Zed"]
        );
        plan.execute().unwrap();
        let output =
            fs::read_to_string(destination.join("v1/groups/Alpha/ObjectData.xml")).unwrap();
        let repaired_target = root
            .0
            .join("relative/game.exe")
            .to_string_lossy()
            .into_owned();
        let repaired_working_directory = root.0.join("relative").to_string_lossy().into_owned();
        assert!(output.contains(&repaired_target));
        assert!(output.contains(&format!("{repaired_working_directory}</WorkingDirectory>")));
        assert!(output.contains("shell:AppsFolder\\Foo!Bar"));
        assert!(root.0.join("config/Alpha/ObjectData.xml").is_file());
    }

    #[test]
    fn portable_policy_preserves_relative_values() {
        let root = TempDir::new();
        write_group(&root.0, "Games", XML);
        let plan = LegacyMigrationPlan::discover_with_policy(
            &root.0,
            root.0.join("out"),
            PathRepairPolicy::Portable,
        )
        .unwrap();
        plan.execute().unwrap();
        let output = fs::read_to_string(root.0.join("out/v1/groups/Games/ObjectData.xml")).unwrap();
        assert!(output.contains("relative/game.exe"));
        assert!(!output.contains(
            &root
                .0
                .join("relative/game.exe")
                .to_string_lossy()
                .to_string()
        ));
    }

    #[test]
    fn failure_after_staging_a_previous_group_leaves_no_partial_output() {
        let root = TempDir::new();
        write_group(&root.0, "Alpha", XML.replace("Games", "Alpha").as_str());
        write_group(&root.0, "Zed", XML.replace("Games", "Zed").as_str());
        let destination = root.0.join("out");
        let plan = LegacyMigrationPlan::discover(&root.0, &destination).unwrap();
        fs::remove_file(root.0.join("config/Zed/ObjectData.xml")).unwrap();
        assert!(matches!(plan.execute(), Err(MigrationError::Io { .. })));
        assert!(!destination.exists());
        assert!(!root.0.join(".migration").exists());
    }

    #[test]
    fn rejects_invalid_categories_and_existing_destination() {
        let root = TempDir::new();
        write_group(
            &root.0,
            "escape",
            XML.replace("<Name>Games</Name>", "<Name>../escape</Name>")
                .as_str(),
        );
        assert!(matches!(
            LegacyMigrationPlan::discover(&root.0, root.0.join("out")),
            Err(MigrationError::InvalidCategory { .. })
        ));
        let root = TempDir::new();
        write_group(&root.0, "Games", XML);
        let destination = root.0.join("out");
        fs::create_dir_all(destination.join("v1")).unwrap();
        let plan = LegacyMigrationPlan::discover(&root.0, &destination).unwrap();
        assert!(matches!(
            plan.execute(),
            Err(MigrationError::Collision { .. })
        ));
    }

    #[test]
    fn backup_can_restore_the_legacy_config_after_a_failed_migration() {
        let root = TempDir::new();
        write_group(&root.0, "Games", XML);
        let destination = root.0.join("out");
        let plan = LegacyMigrationPlan::discover_with_policy(
            &root.0,
            &destination,
            PathRepairPolicy::Portable,
        )
        .unwrap();
        let report = plan.execute_with_backup().unwrap();
        assert!(report.backup_path.join("Games/ObjectData.xml").is_file());
        fs::write(root.0.join("config/Games/ObjectData.xml"), "changed").unwrap();
        plan.restore_backup(&report.backup_path).unwrap();
        assert_eq!(
            fs::read_to_string(root.0.join("config/Games/ObjectData.xml")).unwrap(),
            XML
        );
    }

    #[test]
    fn installed_root_is_user_data_and_portable_root_is_explicit() {
        let portable = root_policy_path(DataRootPolicy::Portable(PathBuf::from("portable")));
        assert_eq!(portable, PathBuf::from("portable"));
        let installed = resolve_data_root(DataRootPolicy::Installed {
            application_name: "Taskbar Groups".into(),
        })
        .unwrap();
        assert_eq!(installed.file_name().unwrap(), "Taskbar_Groups");
        assert_ne!(
            installed,
            std::env::current_exe().unwrap().parent().unwrap()
        );
    }

    fn root_policy_path(policy: DataRootPolicy) -> PathBuf {
        resolve_data_root(policy).unwrap()
    }
}
