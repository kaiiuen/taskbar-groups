//! Filesystem locations and persistence contracts for legacy taskbar groups.

pub mod migration;

use std::{
    env, fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use crate::domain::Category;

pub const CONFIG_DIRECTORY: &str = "config";
pub const JIT_DIRECTORY: &str = "JITComp";
pub const SHORTCUTS_DIRECTORY: &str = "Shortcuts";
pub const OBJECT_DATA_FILE: &str = "ObjectData.xml";
pub const GROUP_IMAGE_FILE: &str = "GroupImage.png";
pub const GROUP_ICON_FILE: &str = "GroupIcon.ico";
pub const ICONS_DIRECTORY: &str = "Icons";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub root: PathBuf,
    pub jit_comp: PathBuf,
    pub config: PathBuf,
    pub shortcuts: PathBuf,
}

impl AppPaths {
    /// Locate the portable application layout beside the running executable.
    pub fn beside_executable() -> io::Result<Self> {
        let executable = env::current_exe()?;
        let root = executable
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_owned();
        Ok(Self::from_root(root))
    }

    /// Build paths from an explicit root. This keeps persistence testable and
    /// also supports portable installs without changing the on-disk layout.
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            jit_comp: root.join(JIT_DIRECTORY),
            config: root.join(CONFIG_DIRECTORY),
            shortcuts: root.join(SHORTCUTS_DIRECTORY),
            root,
        }
    }

    /// Create the directories the legacy application creates during startup.
    pub fn ensure_directories(&self) -> io::Result<()> {
        fs::create_dir_all(&self.jit_comp)?;
        fs::create_dir_all(&self.config)?;
        fs::create_dir_all(&self.shortcuts)
    }

    /// Application-facing migration boundary. The caller chooses the legacy
    /// source and portable/installed path policy; persistence owns all I/O.
    pub fn migrate_legacy(
        &self,
        legacy_root: impl Into<PathBuf>,
        policy: migration::PathRepairPolicy,
    ) -> Result<migration::LegacyMigrationPlan, migration::MigrationError> {
        let plan = migration::LegacyMigrationPlan::discover_with_policy(
            legacy_root,
            self.root.join("migrated-data"),
            policy,
        )?;
        plan.execute()?;
        Ok(plan)
    }

    pub fn group(&self, name: &str) -> io::Result<GroupPaths> {
        let stored_name = stored_group_name(name)?;
        Ok(GroupPaths::new(self, stored_name))
    }

    pub fn load_group(&self, name: &str) -> io::Result<Category> {
        self.group(name)?.load()
    }

    pub fn save_group(&self, category: &Category) -> io::Result<GroupPaths> {
        let group = self.group(&category.name)?;
        #[cfg(windows)]
        if category.icon_source.as_ref().is_some_and(|icon| {
            let path = std::path::Path::new(&icon.path);
            path.is_file() || path.is_dir()
        }) {
            let previous = group.load().ok();
            crate::assets::GroupAssetStore::new(
                crate::platform::icon_cache::PlatformIconExtractor,
                32,
            )
            .map_err(asset_io_error)?
            .synchronize(&group, category, previous.as_ref())
            .map_err(asset_io_error)?;
        }
        group.save(category)?;
        Ok(group)
    }

    /// Persistence seam for portable tests and alternate platform adapters.
    pub fn save_group_with_assets<E: crate::platform::icon_cache::IconExtractor>(
        &self,
        category: &Category,
        store: &crate::assets::GroupAssetStore<E>,
    ) -> Result<GroupPaths, SaveError> {
        let group = self.group(&category.name).map_err(SaveError::Io)?;
        let previous = group.load().ok();
        store
            .synchronize(&group, category, previous.as_ref())
            .map_err(SaveError::Asset)?;
        group.save(category).map_err(SaveError::Io)?;
        Ok(group)
    }

    pub fn delete_group(&self, name: &str) -> io::Result<()> {
        let group = self.group(name)?;
        if group.directory.exists() {
            fs::remove_dir_all(&group.directory)?;
        }
        let shortcut = group.shortcut_path(self);
        if shortcut.exists() {
            fs::remove_file(shortcut)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPaths {
    pub directory: PathBuf,
    pub object_data: PathBuf,
    pub group_image: PathBuf,
    pub group_icon: PathBuf,
    pub icons: PathBuf,
}

impl GroupPaths {
    fn new(app: &AppPaths, stored_name: String) -> Self {
        let directory = app.config.join(stored_name);
        Self {
            object_data: directory.join(OBJECT_DATA_FILE),
            group_image: directory.join(GROUP_IMAGE_FILE),
            group_icon: directory.join(GROUP_ICON_FILE),
            icons: directory.join(ICONS_DIRECTORY),
            directory,
        }
    }

    pub fn shortcut_file_name(&self) -> &str {
        self.directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    }

    pub fn shortcut_path(&self, app: &AppPaths) -> PathBuf {
        app.shortcuts.join(format!(
            "{}.lnk",
            self.shortcut_file_name().replace('_', " ")
        ))
    }

    pub fn load(&self) -> io::Result<Category> {
        let xml = fs::read_to_string(&self.object_data)?;
        Category::from_legacy_xml(&xml).map_err(|error| {
            io::Error::new(
                ErrorKind::InvalidData,
                format!("cannot parse {}: {error:?}", self.object_data.display()),
            )
        })
    }

    pub fn save(&self, category: &Category) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        fs::write(&self.object_data, category.to_legacy_xml())
    }
}

#[derive(Debug)]
pub enum SaveError {
    Io(io::Error),
    Asset(crate::assets::AssetError),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Asset(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for SaveError {}

fn asset_io_error(error: crate::assets::AssetError) -> io::Error {
    io::Error::new(ErrorKind::Other, error.to_string())
}

fn stored_group_name(name: &str) -> io::Result<String> {
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
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "group name must be a non-empty path component",
        ));
    }
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Category, ProgramShortcut};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("taskbar-groups-{nonce}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn layout_matches_legacy_startup_and_group_paths() {
        let root = TestRoot::new();
        let app = AppPaths::from_root(&root.0);
        app.ensure_directories().unwrap();

        assert!(app.jit_comp.is_dir());
        assert!(app.config.is_dir());
        assert!(app.shortcuts.is_dir());
        let group = app.group("My   Group").unwrap();
        assert_eq!(group.directory, root.0.join("config/My_Group"));
        assert_eq!(group.object_data, group.directory.join("ObjectData.xml"));
        assert_eq!(
            group.shortcut_path(&app),
            root.0.join("Shortcuts/My Group.lnk")
        );
    }

    #[test]
    fn save_and_load_preserve_legacy_xml_values() {
        let root = TestRoot::new();
        let app = AppPaths::from_root(&root.0);
        let mut category = Category::new("My Group");
        category.color_string = "#123456".into();
        category.allow_open_all = true;
        category.width = 4;
        category.opacity = 30.0;
        category
            .shortcut_list
            .push(ProgramShortcut::new(r"C:\Tools & Games\play.exe"));

        let paths = app.save_group(&category).unwrap();
        assert_eq!(paths.directory.file_name().unwrap(), "My_Group");
        assert!(paths.object_data.is_file());
        assert_eq!(app.load_group("My_Group").unwrap(), category);
        assert_eq!(app.load_group("My Group").unwrap(), category);
    }

    #[test]
    fn malformed_xml_is_reported_as_invalid_data() {
        let root = TestRoot::new();
        let app = AppPaths::from_root(&root.0);
        let group = app.group("Broken").unwrap();
        fs::create_dir_all(&group.directory).unwrap();
        fs::write(&group.object_data, "<Category><Name>").unwrap();

        assert_eq!(group.load().unwrap_err().kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn group_names_cannot_escape_config_directory() {
        let root = TestRoot::new();
        let app = AppPaths::from_root(&root.0);
        assert_eq!(
            app.group("../outside").unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(
            app.group("   ").unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
    }

    #[test]
    fn delete_group_removes_config_and_legacy_shortcut() {
        let root = TestRoot::new();
        let app = AppPaths::from_root(&root.0);
        let category = Category::new("My Group");
        let group = app.save_group(&category).unwrap();
        fs::create_dir_all(&app.shortcuts).unwrap();
        fs::write(group.shortcut_path(&app), []).unwrap();

        app.delete_group("My Group").unwrap();
        assert!(!group.directory.exists());
        assert!(!group.shortcut_path(&app).exists());
    }
}
