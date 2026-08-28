use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use taskbar_groups::{
    domain::{Category, ProgramShortcut},
    persistence::{migration::PathRepairPolicy, AppPaths},
    platform::{
        icon_cache::{cache_icon, CachePolicy, IconExtractor, IconSource},
        shell_link::{create_or_update, AppUserModelId, ShellLinkRequest},
        LaunchError, LaunchSpec, Launcher, ResolvedTarget, TargetKind,
    },
    ui::{Action, Controller, UiShell},
};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("taskbar-groups-{label}-{nonce}"));
        fs::create_dir_all(&path).expect("create temporary test root");
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Default)]
struct RecordingShell {
    launched: Rc<RefCell<Vec<LaunchSpec>>>,
    revealed: Rc<RefCell<Vec<String>>>,
    errors: Rc<RefCell<Vec<String>>>,
}

impl UiShell for RecordingShell {
    fn show_message(&mut self, _: &str) {}

    fn show_error(&mut self, message: &str) {
        self.errors.borrow_mut().push(message.to_owned());
    }

    fn launch(&mut self, spec: &LaunchSpec) -> Result<(), std::io::Error> {
        self.launched.borrow_mut().push(spec.clone());
        Ok(())
    }

    fn reveal_group(&mut self, shortcut_path: &str) {
        self.revealed.borrow_mut().push(shortcut_path.to_owned());
    }
}

fn category() -> Category {
    let mut category = Category::new("Games");
    category.allow_open_all = true;
    category.shortcut_list = vec![
        ProgramShortcut {
            file_path: "one.exe".into(),
            name: "One".into(),
            arguments: "--safe".into(),
            working_directory: "work".into(),
            ..ProgramShortcut::default()
        },
        ProgramShortcut {
            file_path: "two.lnk".into(),
            name: "Two".into(),
            ..ProgramShortcut::default()
        },
        ProgramShortcut {
            file_path: "store-id".into(),
            is_windows_app: true,
            name: "Store".into(),
            ..ProgramShortcut::default()
        },
    ];
    category
}

#[test]
fn persistence_controller_planning_and_recording_shell_form_one_flow() {
    let root = TempRoot::new("controller-flow");
    let paths = AppPaths::from_root(&root.0);
    let saved = category();
    paths.save_group(&saved).expect("persist category");

    let shell = RecordingShell::default();
    let launched = shell.launched.clone();
    let revealed = shell.revealed.clone();
    let mut controller = Controller::new(
        taskbar_groups::platform::LaunchRequest {
            group_name: Some("Games".into()),
        },
        paths.clone(),
        shell,
    );

    assert_eq!(controller.view().editor.as_ref(), Some(&saved));
    controller.dispatch(Action::Key('2'));
    controller.dispatch(Action::CtrlEnter);
    controller.dispatch(Action::RevealGroup("Games".into()));

    let launched = launched.borrow();
    assert_eq!(launched.len(), 4);
    assert_eq!(
        launched[0].target,
        ResolvedTarget::Path {
            path: "two.lnk".into(),
            kind: TargetKind::ShellLink,
        }
    );
    assert_eq!(launched[0].arguments, "");
    assert_eq!(
        launched[1].target,
        ResolvedTarget::Path {
            path: "one.exe".into(),
            kind: TargetKind::Executable,
        }
    );
    assert_eq!(
        launched[2].target,
        ResolvedTarget::Path {
            path: "two.lnk".into(),
            kind: TargetKind::ShellLink,
        }
    );
    assert_eq!(
        launched[3].target,
        ResolvedTarget::WindowsApp {
            app_user_model_id: "store-id".into(),
        }
    );
    assert_eq!(revealed.borrow().len(), 1);
    assert!(
        revealed.borrow()[0].ends_with("Shortcuts\\Games.lnk")
            || revealed.borrow()[0].ends_with("Shortcuts/Games.lnk")
    );
}

#[test]
fn controller_save_round_trip_reloads_the_persisted_group() {
    let root = TempRoot::new("controller-save");
    let paths = AppPaths::from_root(&root.0);
    let shell = RecordingShell::default();
    let mut controller = Controller::new(
        taskbar_groups::platform::LaunchRequest { group_name: None },
        paths.clone(),
        shell,
    );
    controller.dispatch(Action::BeginNewGroup);
    controller.dispatch(Action::SetGroupName("Saved Group".into()));
    controller.dispatch(Action::AddShortcut {
        path: "app.exe".into(),
        is_windows_app: false,
    });
    controller.dispatch(Action::SetIcon {
        path: "icon.ico".into(),
    });
    controller.dispatch(Action::SaveGroup);

    assert!(controller.view().error.is_none());
    assert_eq!(controller.view().groups, vec!["Saved_Group"]);
    assert_eq!(
        paths.load_group("Saved Group").expect("reload group").name,
        "Saved Group"
    );
}

#[test]
fn migration_output_is_loadable_and_preserves_legacy_source() {
    let root = TempRoot::new("migration-flow");
    let legacy = root.0.join("legacy");
    let source_group = legacy.join("config/Legacy");
    fs::create_dir_all(&source_group).expect("create legacy group");
    let source = category().to_legacy_xml();
    fs::write(source_group.join("ObjectData.xml"), source).expect("write legacy XML");

    let app = AppPaths::from_root(root.0.join("portable"));
    let plan = app
        .migrate_legacy(&legacy, PathRepairPolicy::Portable)
        .expect("migrate legacy data");
    assert_eq!(plan.items.len(), 1);
    assert!(source_group.join("ObjectData.xml").is_file());
    let migrated = plan.destination_root.join("v1/groups/Games/ObjectData.xml");
    let migrated_category =
        Category::from_legacy_xml(&fs::read_to_string(migrated).expect("read migrated XML"))
            .expect("parse migrated XML");
    assert_eq!(migrated_category, category());
}

#[derive(Debug)]
struct RecordingExtractor;

impl IconExtractor for RecordingExtractor {
    fn extract(
        &self,
        _: &IconSource,
        size: u32,
    ) -> Result<Vec<u8>, taskbar_groups::platform::icon_cache::IconCacheError> {
        Ok(format!("fake-icon-{size}").into_bytes())
    }
}

#[test]
fn icon_cache_boundary_uses_fake_extractor_and_publishes_cache_file() {
    let root = TempRoot::new("icon-flow");
    let target = root.0.join("game.exe");
    fs::write(&target, b"not an executable").expect("create fake icon target");
    let policy = CachePolicy::new(root.0.join("icons"), 32).expect("cache policy");
    let entry = cache_icon(
        &policy,
        &RecordingExtractor,
        "Games",
        &IconSource::Executable(target),
    )
    .expect("cache icon");
    assert!(entry.path.is_file());
    assert_eq!(fs::read(entry.path).expect("read cache"), b"fake-icon-32");
}

#[test]
fn shell_link_boundary_dry_run_keeps_request_platform_neutral() {
    let request = ShellLinkRequest {
        link_path: PathBuf::from("Games.lnk"),
        target: PathBuf::from("game.exe"),
        arguments: "--safe".into(),
        working_directory: Some(PathBuf::from("work")),
        icon_path: Some(PathBuf::from("game.exe")),
        icon_index: 0,
        description: "Games".into(),
        app_user_model_id: AppUserModelId::legacy_group("Games").expect("identity"),
        dry_run: true,
    };
    let result = create_or_update(&request).expect("dry-run shell link");
    assert!(!result.written);
    assert_eq!(
        result.app_user_model_id,
        "tjackenpacken.taskbarGroup.menu.Games"
    );
    assert!(!Path::new("Games.lnk").exists());
}

#[derive(Debug, Default)]
struct RecordingLauncher(Rc<RefCell<Vec<LaunchSpec>>>);

impl Launcher for RecordingLauncher {
    fn launch(&self, spec: &LaunchSpec) -> Result<(), LaunchError> {
        self.0.borrow_mut().push(spec.clone());
        Ok(())
    }
}

#[test]
fn launcher_trait_records_plans_without_platform_side_effects() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let launcher = RecordingLauncher(calls.clone());
    let spec = LaunchSpec {
        target: ResolvedTarget::Path {
            path: "folder".into(),
            kind: TargetKind::Folder,
        },
        arguments: "".into(),
        working_directory: "".into(),
    };
    launcher.launch(&spec).expect("record launch");
    assert_eq!(calls.borrow().as_slice(), std::slice::from_ref(&spec));
}

#[cfg(windows)]
mod windows_smoke {
    use super::*;

    use taskbar_groups::platform::{TargetKind, WindowsPlatform};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

    fn system_root() -> PathBuf {
        std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
    }

    #[test]
    fn harmless_real_executable_launch_skips_without_system_command() {
        let target = system_root().join("System32/cmd.exe");
        if !target.is_file() {
            eprintln!("SKIP: cmd.exe is unavailable");
            return;
        }
        let spec = LaunchSpec {
            target: ResolvedTarget::Path {
                path: target.to_string_lossy().into_owned(),
                kind: TargetKind::Executable,
            },
            arguments: "/c exit 0".into(),
            working_directory: system_root().to_string_lossy().into_owned(),
        };
        WindowsPlatform
            .launch(&spec)
            .expect("launch harmless command");
    }

    #[test]
    fn temporary_shell_link_is_written_or_skipped_for_environment() {
        let target = system_root().join("System32/cmd.exe");
        if !target.is_file() {
            eprintln!("SKIP: cmd.exe is unavailable");
            return;
        }
        let link = std::env::temp_dir().join(format!(
            "taskbar-groups-integration-{}.lnk",
            std::process::id()
        ));
        let mut request =
            ShellLinkRequest::for_group("Integration", &link, &target).expect("link request");
        request.arguments = "/c exit 0".into();
        request.working_directory = Some(system_root());
        match create_or_update(&request) {
            Ok(result) => {
                assert!(result.written && link.is_file());
                fs::remove_file(link).expect("remove temporary link");
            }
            Err(taskbar_groups::platform::shell_link::ShellLinkError::Com { .. })
            | Err(taskbar_groups::platform::shell_link::ShellLinkError::Io(_)) => {
                eprintln!("SKIP: Windows shell-link environment unavailable");
            }
            Err(error) => panic!("unexpected shell-link error: {error}"),
        }
    }

    #[test]
    fn folder_shell_launch_skips_without_interactive_shell() {
        let root = TempRoot::new("folder-shell");
        let spec = LaunchSpec {
            target: ResolvedTarget::Path {
                path: root.0.to_string_lossy().into_owned(),
                kind: TargetKind::Folder,
            },
            arguments: String::new(),
            working_directory: String::new(),
        };
        if unsafe { GetDesktopWindow() }.is_null() {
            eprintln!("SKIP: no interactive Windows desktop");
            return;
        }
        match WindowsPlatform.launch(&spec) {
            Ok(()) => {}
            Err(LaunchError::Shell { .. }) => eprintln!("SKIP: Windows shell unavailable"),
            Err(error) => panic!("unexpected folder launch error: {error}"),
        }
    }

    #[test]
    fn native_ui_availability_skips_without_desktop() {
        if unsafe { GetDesktopWindow() }.is_null() {
            eprintln!("SKIP: native UI requires an interactive desktop");
            return;
        }
        assert!(!unsafe { GetDesktopWindow() }.is_null());
    }
}
