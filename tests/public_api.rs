use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use taskbar_groups::{
    domain::{Category, ProgramShortcut},
    persistence::AppPaths,
    platform::{
        icon_cache::{rebuild_group_cache, CachePolicy, IconCacheError, IconExtractor, IconSource},
        LaunchMode, LaunchPlanner, LaunchRequest, LaunchTrigger, PassthroughResolver, PlanError,
        ResolveError, ResolvedTarget, ShortcutNumber, ShortcutResolver, TargetKind,
    },
    ui::{action_for_event, Action, NativeEvent},
};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("taskbar-groups-public-{label}-{nonce}"));
        fs::create_dir_all(&path).expect("create temporary test root");
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn shortcut(path: &str) -> ProgramShortcut {
    ProgramShortcut {
        file_path: path.into(),
        arguments: "--safe".into(),
        working_directory: "work".into(),
        ..ProgramShortcut::default()
    }
}

#[test]
fn launch_request_parses_group_mode_and_app_identity() {
    let configure = LaunchRequest::from_args(["taskbar-groups.exe"]);
    assert_eq!(configure.group_name, None);
    assert_eq!(configure.mode(), LaunchMode::Configure);
    assert_eq!(
        configure.app_user_model_id_for_group(),
        "tjackenpacken.taskbarGroup.main"
    );

    let group = LaunchRequest::from_args(["app.exe", "Games", "ignored"]);
    assert_eq!(
        group.mode(),
        LaunchMode::Group {
            name: "Games".into()
        }
    );
    assert_eq!(
        group.app_user_model_id_for_group(),
        "tjackenpacken.taskbarGroup.menu.Games"
    );
}

#[test]
fn launch_request_uses_only_the_first_argument_after_executable() {
    let request = LaunchRequest::from_args(["app.exe", "Games", "Other"]);
    assert_eq!(request.group_name.as_deref(), Some("Games"));
    assert_eq!(
        request.mode(),
        LaunchMode::Group {
            name: "Games".into()
        }
    );
}

#[test]
fn shortcut_number_maps_one_through_zero_and_rejects_other_keys() {
    for (key, index) in [('1', 0), ('2', 1), ('9', 8), ('0', 9)] {
        assert_eq!(
            ShortcutNumber::from_key(key).expect("shortcut key").index(),
            index
        );
    }
    assert_eq!(ShortcutNumber::from_key('a'), None);
    assert_eq!(ShortcutNumber::from_key('!'), None);
}

#[test]
fn target_kind_classifies_extensions_apps_and_existing_folders() {
    let root = TempRoot::new("target-kind");
    assert_eq!(
        TargetKind::classify("GAME.EXE", false),
        TargetKind::Executable
    );
    assert_eq!(
        TargetKind::classify("game.LNK", false),
        TargetKind::ShellLink
    );
    assert_eq!(
        TargetKind::classify("web.URL", false),
        TargetKind::UrlShortcut
    );
    assert_eq!(
        TargetKind::classify("store-entry", true),
        TargetKind::WindowsApp
    );
    assert_eq!(
        TargetKind::classify(&root.0.to_string_lossy(), false),
        TargetKind::Folder
    );
    assert_eq!(TargetKind::classify("notes.txt", false), TargetKind::Other);
}

#[test]
fn passthrough_resolver_preserves_paths_and_windows_app_ids() {
    let resolver = PassthroughResolver;
    assert_eq!(
        resolver
            .resolve(&shortcut("game.lnk"))
            .expect("resolve link"),
        ResolvedTarget::Path {
            path: "game.lnk".into(),
            kind: TargetKind::ShellLink,
        }
    );
    let mut app = shortcut("Microsoft.Game_123!App");
    app.is_windows_app = true;
    assert_eq!(
        resolver.resolve(&app).expect("resolve Windows app"),
        ResolvedTarget::WindowsApp {
            app_user_model_id: "Microsoft.Game_123!App".into(),
        }
    );
    assert_eq!(
        resolver.resolve(&shortcut("   ")).unwrap_err(),
        ResolveError::EmptyPath
    );
}

#[derive(Debug, Default, Clone, Copy)]
struct RecordingResolver;

impl ShortcutResolver for RecordingResolver {
    fn resolve(&self, shortcut: &ProgramShortcut) -> Result<ResolvedTarget, ResolveError> {
        Ok(ResolvedTarget::Path {
            path: shortcut.file_path.clone(),
            kind: TargetKind::Other,
        })
    }
}

#[test]
fn launch_planner_plans_number_and_open_all_with_arguments() {
    let mut category = Category::new("Tools");
    category.allow_open_all = true;
    category.shortcut_list = vec![shortcut("one"), shortcut("two")];
    let planner = LaunchPlanner::new(RecordingResolver);

    let one = planner
        .plan(
            &category,
            LaunchTrigger::Number(ShortcutNumber::OneBased(1)),
        )
        .expect("plan one");
    assert_eq!(one.len(), 1);
    assert_eq!(
        one[0].target,
        ResolvedTarget::Path {
            path: "one".into(),
            kind: TargetKind::Other
        }
    );
    assert_eq!(one[0].arguments, "--safe");
    assert_eq!(one[0].working_directory, "work");

    let all = planner
        .plan(&category, LaunchTrigger::CtrlEnter)
        .expect("plan all");
    assert_eq!(all.len(), 2);
}

#[test]
fn launch_planner_handles_disabled_open_all_and_missing_number() {
    let mut category = Category::new("Tools");
    category.shortcut_list.push(shortcut("one"));
    let planner = LaunchPlanner::new(RecordingResolver);
    assert_eq!(
        planner.plan(&category, LaunchTrigger::CtrlEnter).unwrap(),
        Vec::new()
    );
    assert_eq!(
        planner
            .plan(
                &category,
                LaunchTrigger::Number(ShortcutNumber::OneBased(2))
            )
            .unwrap_err(),
        PlanError::NoShortcut { index: 1 }
    );
}

#[derive(Debug, Clone)]
struct FakeExtractor {
    calls: Rc<RefCell<Vec<(IconSource, u32)>>>,
}

impl IconExtractor for FakeExtractor {
    fn extract(&self, source: &IconSource, size: u32) -> Result<Vec<u8>, IconCacheError> {
        self.calls.borrow_mut().push((source.clone(), size));
        Ok(format!("icon-{size}-{}", self.calls.borrow().len()).into_bytes())
    }
}

#[test]
fn rebuild_group_cache_removes_stale_entries_and_extracts_each_source() {
    let root = TempRoot::new("icon-rebuild");
    let executable = root.0.join("one.exe");
    let folder = root.0.join("folder");
    fs::write(&executable, b"fake").expect("create executable target");
    fs::create_dir(&folder).expect("create folder target");
    let policy = CachePolicy::new(root.0.join("icons"), 48).expect("cache policy");
    let calls = Rc::new(RefCell::new(Vec::new()));
    let extractor = FakeExtractor {
        calls: calls.clone(),
    };
    let stale = policy
        .group_path("Tools")
        .expect("group path")
        .join("stale.ico");
    fs::create_dir_all(stale.parent().expect("stale parent")).expect("create cache directory");
    fs::write(&stale, b"stale").expect("write stale cache");

    let sources = vec![
        IconSource::Executable(executable),
        IconSource::Folder(folder),
    ];
    let results = rebuild_group_cache(&policy, &extractor, "Tools", &sources);
    assert!(results.iter().all(Result::is_ok));
    assert!(!stale.exists());
    assert_eq!(calls.borrow().len(), 2);
    for result in results {
        assert_eq!(
            fs::read(result.expect("cache entry").path)
                .expect("read icon")
                .len(),
            9
        );
    }
}

#[test]
fn native_events_translate_to_public_controller_actions() {
    assert_eq!(
        action_for_event(NativeEvent::NewGroup),
        Action::BeginNewGroup
    );
    assert_eq!(
        action_for_event(NativeEvent::AddShortcut {
            path: "app.exe".into(),
            is_windows_app: true
        }),
        Action::AddShortcut {
            path: "app.exe".into(),
            is_windows_app: true
        }
    );
    assert_eq!(
        action_for_event(NativeEvent::Icon("icon.ico".into())),
        Action::SetIcon {
            path: "icon.ico".into()
        }
    );
    assert_eq!(action_for_event(NativeEvent::CtrlEnter), Action::CtrlEnter);
}

#[test]
fn app_paths_expose_portable_layout_and_safe_group_shortcut_path() {
    let root = TempRoot::new("paths");
    let paths = AppPaths::from_root(&root.0);
    assert_eq!(paths.jit_comp, root.0.join("JITComp"));
    assert_eq!(paths.config, root.0.join("config"));
    assert_eq!(paths.shortcuts, root.0.join("Shortcuts"));
    paths.ensure_directories().expect("create app directories");
    assert!(paths.jit_comp.is_dir() && paths.config.is_dir() && paths.shortcuts.is_dir());

    let group = paths.group("My Group").expect("group paths");
    assert_eq!(group.shortcut_file_name(), "My_Group");
    assert_eq!(
        group.shortcut_path(&paths),
        root.0.join("Shortcuts/My Group.lnk")
    );
    assert!(paths.group("../escape").is_err());
    assert!(!Path::new("My Group.lnk").exists());
}
