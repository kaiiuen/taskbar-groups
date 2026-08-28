use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use taskbar_groups::{
    domain::{Category, ProgramShortcut},
    persistence::migration::{
        resolve_data_root, DataRootPolicy, LegacyMigrationPlan, MigrationError, PathRepairPolicy,
    },
};

struct TempTree(PathBuf);

impl TempTree {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("taskbar-groups-migration-{label}-{nonce}"));
        fs::create_dir_all(&path).expect("create disposable migration root");
        Self(path)
    }

    fn legacy(&self) -> PathBuf {
        self.0.join("legacy")
    }

    fn destination(&self, label: &str) -> PathBuf {
        self.0.join(label)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_group(root: &Path, directory: &str, category: &Category) -> PathBuf {
    let group = root.join("config").join(directory);
    fs::create_dir_all(&group).expect("create legacy group");
    let source = group.join("ObjectData.xml");
    fs::write(&source, category.to_legacy_xml()).expect("write legacy group");
    source
}

fn category(name: &str, target: &str, working_directory: &str) -> Category {
    let mut category = Category::new(name);
    category.shortcut_list.push(ProgramShortcut {
        file_path: target.into(),
        working_directory: working_directory.into(),
        ..ProgramShortcut::default()
    });
    category
}

fn migrated_file(destination: &Path, name: &str) -> PathBuf {
    destination
        .join("v1/groups")
        .join(name)
        .join("ObjectData.xml")
}

#[test]
fn installed_root_uses_a_user_writable_application_data_location() {
    let root = resolve_data_root(DataRootPolicy::Installed {
        application_name: "Taskbar Groups Acceptance".into(),
    })
    .expect("resolve installed data root");
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".local/share").into_os_string())
        })
        .expect("test environment provides a user application-data location");

    assert_eq!(root, PathBuf::from(base).join("Taskbar_Groups_Acceptance"));
    assert_ne!(
        root,
        std::env::current_exe().expect("locate test executable")
    );
}

#[test]
fn migration_imports_multiple_groups_in_stable_order() {
    let tree = TempTree::new("multiple");
    let legacy = tree.legacy();
    write_group(&legacy, "z-source", &category("Z Group", "z.exe", "z-work"));
    write_group(&legacy, "a-source", &category("A Group", "a.exe", "a-work"));
    let destination = tree.destination("destination");

    let plan = LegacyMigrationPlan::discover_for_root_policy(
        &legacy,
        DataRootPolicy::Portable(destination.clone()),
        PathRepairPolicy::Portable,
    )
    .expect("discover multiple groups");
    assert_eq!(
        plan.items
            .iter()
            .map(|item| item.group_name.as_str())
            .collect::<Vec<_>>(),
        ["A Group", "Z Group"]
    );
    plan.execute().expect("publish multiple groups");
    assert!(migrated_file(&destination, "A_Group").is_file());
    assert!(migrated_file(&destination, "Z_Group").is_file());
}

#[test]
fn migration_creates_backup_restores_it_and_preserves_legacy_source() {
    let tree = TempTree::new("backup-restore");
    let legacy = tree.legacy();
    let original = category("Restore Me", "bin/app.exe", "work");
    let source = write_group(&legacy, "Restore", &original);
    let destination = tree.destination("destination");
    let plan = LegacyMigrationPlan::discover(&legacy, &destination).expect("discover source");

    let report = plan.execute_with_backup().expect("migrate with backup");
    assert!(report.backup_path.is_dir());
    assert!(source.is_file());
    assert!(migrated_file(&destination, "Restore_Me").is_file());

    fs::write(
        &source,
        category("Changed", "changed.exe", "changed").to_legacy_xml(),
    )
    .expect("change legacy source");
    plan.restore_backup(&report.backup_path)
        .expect("restore legacy backup");
    assert_eq!(
        fs::read_to_string(source).expect("read restored source"),
        original.to_legacy_xml()
    );
}

#[test]
fn installed_migration_repairs_relative_paths_and_preserves_absolute_paths() {
    let tree = TempTree::new("paths");
    let legacy = tree.legacy();
    let absolute = if cfg!(windows) {
        r"C:\Existing\app.exe"
    } else {
        "/existing/app.exe"
    };
    let source = category("Paths", "bin/app.exe", "work");
    let mut source = source;
    source.shortcut_list.push(ProgramShortcut {
        file_path: absolute.into(),
        working_directory: absolute.into(),
        ..ProgramShortcut::default()
    });
    let source_file = write_group(&legacy, "Paths", &source);
    let destination = tree.destination("destination");
    let plan = LegacyMigrationPlan::discover(&legacy, &destination).expect("discover paths");
    plan.execute().expect("publish repaired paths");

    let migrated = Category::from_legacy_xml(
        &fs::read_to_string(migrated_file(&destination, "Paths")).expect("read migrated paths"),
    )
    .expect("parse migrated paths");
    assert_eq!(
        migrated.shortcut_list[0].file_path,
        legacy.join("bin/app.exe").to_string_lossy()
    );
    assert_eq!(
        migrated.shortcut_list[0].working_directory,
        legacy.join("work").to_string_lossy()
    );
    assert_eq!(migrated.shortcut_list[1].file_path, absolute);
    assert_eq!(migrated.shortcut_list[1].working_directory, absolute);
    assert!(
        source_file.is_file(),
        "migration must not remove its source"
    );
}

#[test]
fn migration_reports_destination_collisions_without_overwriting() {
    let tree = TempTree::new("collision");
    let legacy = tree.legacy();
    write_group(&legacy, "one", &category("Same Name", "one.exe", "work"));
    write_group(&legacy, "two", &category("Same   Name", "two.exe", "work"));
    let destination = tree.destination("destination");

    let error = LegacyMigrationPlan::discover(&legacy, &destination).unwrap_err();
    assert!(matches!(error, MigrationError::Collision { .. }));
    assert!(!destination.exists());
}

#[test]
fn failed_publish_leaves_no_partial_version_and_keeps_recovery_data() {
    let tree = TempTree::new("atomic-failure");
    let legacy = tree.legacy();
    let source = write_group(&legacy, "Failure", &category("Failure", "app.exe", "work"));
    let destination = tree.destination("destination");
    let plan = LegacyMigrationPlan::discover(&legacy, &destination).expect("discover source");
    fs::write(&destination, b"blocking file").expect("block destination publication");

    let error = plan.execute_with_backup().unwrap_err();
    assert!(matches!(error, MigrationError::Io { .. }));
    assert!(!destination.join("v1").exists());
    let recovery_artifacts = fs::read_dir(tree.0.as_path())
        .expect("read disposable root for recovery artifacts")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".migration-backup-")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recovery_artifacts.len(),
        1,
        "failed publication must leave exactly one automatic backup"
    );
    let backup_source = recovery_artifacts[0]
        .path()
        .join("Failure")
        .join("ObjectData.xml");
    assert_eq!(
        fs::read_to_string(backup_source).expect("read automatic backup"),
        category("Failure", "app.exe", "work").to_legacy_xml(),
        "automatic backup must preserve the source needed for recovery"
    );
    assert!(
        source.is_file(),
        "failed publication must preserve its source"
    );
    let staging_artifacts = fs::read_dir(tree.0.as_path())
        .expect("read disposable root for staging artifacts")
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(".migration-") && !name.starts_with(".migration-backup-")
        })
        .collect::<Vec<_>>();
    assert!(
        staging_artifacts.is_empty(),
        "failed publication must clean its staging directory: {staging_artifacts:?}"
    );
}

#[test]
fn invalid_destination_is_reported_without_mutating_legacy_data() {
    let tree = TempTree::new("error-recovery");
    let legacy = tree.legacy();
    let source = write_group(&legacy, "Error", &category("Error", "app.exe", "work"));
    let destination = tree.destination("not-a-directory");
    fs::write(&destination, b"blocking file").expect("create invalid destination");
    let before = fs::read_to_string(&source).expect("read original source");
    let plan = LegacyMigrationPlan::discover(&legacy, &destination).expect("discover source");

    let error = plan.execute_with_backup().unwrap_err();
    assert!(matches!(error, MigrationError::Io { .. }));
    assert_eq!(
        fs::read_to_string(source).expect("read preserved source"),
        before
    );
    assert!(!destination.join("v1").exists());
}

#[test]
fn missing_backup_is_a_recoverable_error_and_leaves_config_untouched() {
    let tree = TempTree::new("restore-error");
    let legacy = tree.legacy();
    let source = write_group(&legacy, "Restore", &category("Restore", "app.exe", "work"));
    let destination = tree.destination("destination");
    let plan = LegacyMigrationPlan::discover(&legacy, &destination).expect("discover source");
    let before = fs::read_to_string(&source).expect("read source");

    let error = plan
        .restore_backup(&tree.0.join("missing-backup"))
        .unwrap_err();
    assert!(matches!(error, MigrationError::Recovery { .. }));
    assert_eq!(
        fs::read_to_string(source).expect("read untouched source"),
        before
    );
}
