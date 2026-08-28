//! Acceptance coverage for the supported target families and their adapters.
//!
//! The portable tests stop at planning, persistence, and fake adapters. Windows
//! smoke tests only validate harmless shell-link creation or an immediate
//! `cmd.exe /c exit 0` launch and skip when the host cannot provide the surface.

use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use taskbar_groups::{
    domain::{Category, ProgramShortcut},
    persistence::AppPaths,
    platform::{
        icon_cache::{cache_icon, CachePolicy, IconCacheError, IconExtractor, IconSource},
        shell_link::{create_or_update, AppUserModelId, ShellLinkOptions, ShellLinkRequest},
        special_targets::{self, ElevationPolicy, SpecialTargetKind},
        windows_apps::{
            apps_folder_target, FakeWindowsAppDiscovery, WindowsApp, WindowsAppDiscovery,
            WindowsAppError,
        },
        LaunchPlanner, LaunchRequest, LaunchSpec, LaunchTrigger, PassthroughResolver, PlanError,
        ResolveError, ResolvedTarget, ShortcutNumber, ShortcutResolver, TargetKind,
    },
};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("taskbar-groups-target-{label}-{nonce}"));
        fs::create_dir_all(&path).expect("create temporary root");
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn shortcut(path: &str) -> ProgramShortcut {
    ProgramShortcut::new(path)
}

#[test]
fn target_families_classify_executables_links_urls_folders_and_missing_paths() {
    let root = TempRoot::new("families");
    let folder = root.0.join("folder");
    fs::create_dir(&folder).expect("create folder target");
    assert_eq!(
        TargetKind::classify("APP.EXE", false),
        TargetKind::Executable
    );
    assert_eq!(
        TargetKind::classify("shortcut.LNK", false),
        TargetKind::ShellLink
    );
    assert_eq!(
        TargetKind::classify("internet.URL", false),
        TargetKind::UrlShortcut
    );
    assert_eq!(
        TargetKind::classify(&folder.to_string_lossy(), false),
        TargetKind::Folder
    );
    assert_eq!(
        TargetKind::classify("missing-target.exe", false),
        TargetKind::Executable
    );
    assert_eq!(
        TargetKind::classify("missing-target", false),
        TargetKind::Other
    );
}

#[test]
fn special_targets_cover_steam_browser_pwa_office_and_shell_namespace_uris() {
    let cases = [
        (
            " steam://rungameid/730 ",
            SpecialTargetKind::SteamGame {
                app_id: "730".into(),
            },
            "steam://rungameid/730",
        ),
        (
            "https://example.test/app",
            SpecialTargetKind::BrowserUrl,
            "https://example.test/app",
        ),
        (
            "microsoft-edge:https://example.test",
            SpecialTargetKind::PwaUrl,
            "microsoft-edge:https://example.test",
        ),
        (
            "ms-word:ofe|u|https://example.test/file.docx",
            SpecialTargetKind::ShellUri {
                scheme: "ms-word".into(),
            },
            "ms-word:ofe|u|https://example.test/file.docx",
        ),
        (
            "shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            SpecialTargetKind::ShellUri {
                scheme: "shell".into(),
            },
            "shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
        ),
    ];
    for (target, kind, normalized) in cases {
        let planned = special_targets::plan(
            target,
            "--profile=\"safe mode\"",
            "  C:\\Work  ",
            ElevationPolicy::Never,
        )
        .expect("special target plan")
        .expect("special target");
        assert_eq!(planned.kind, kind);
        assert_eq!(planned.target, normalized);
        assert_eq!(planned.arguments, "--profile=\"safe mode\"");
        assert_eq!(planned.working_directory, "C:\\Work");
        assert_eq!(planned.elevation, ElevationPolicy::Never);
    }
    assert_eq!(
        apps_folder_target("Contoso.Office_123!Word"),
        "shell:AppsFolder\\Contoso.Office_123!Word"
    );
    assert!(
        special_targets::plan("document.url", "", "", ElevationPolicy::Never)
            .unwrap()
            .is_some()
    );
}

#[test]
fn special_target_plans_make_elevation_opt_in_and_reject_unsafe_input() {
    let normal = special_targets::plan("https://example.test", "", "", ElevationPolicy::Never)
        .unwrap()
        .unwrap();
    let elevated = special_targets::plan("https://example.test", "", "", ElevationPolicy::RunAs)
        .unwrap()
        .unwrap();
    assert_eq!(normal.elevation, ElevationPolicy::Never);
    assert_eq!(elevated.elevation, ElevationPolicy::RunAs);
    assert!(matches!(
        special_targets::plan("steam://run/not-a-number", "", "", ElevationPolicy::Never),
        Err(special_targets::SpecialTargetError::InvalidSteamUri { .. })
    ));
    assert!(matches!(
        special_targets::plan("https://example.test\0", "", "", ElevationPolicy::Never),
        Err(special_targets::SpecialTargetError::EmbeddedNul { field: "target" })
    ));
}

#[derive(Debug, Default, Clone, Copy)]
struct RecordingResolver;

impl ShortcutResolver for RecordingResolver {
    fn resolve(&self, shortcut: &ProgramShortcut) -> Result<ResolvedTarget, ResolveError> {
        Ok(ResolvedTarget::Path {
            path: shortcut.file_path.clone(),
            kind: TargetKind::classify(&shortcut.file_path, false),
        })
    }
}

#[test]
fn planner_preserves_persisted_order_arguments_and_working_directories() {
    let mut category = Category::new("Compatibility");
    category.allow_open_all = true;
    category.shortcut_list = vec![
        ProgramShortcut {
            file_path: "first.exe".into(),
            arguments: "--one \"two words\"".into(),
            working_directory: " first-dir ".into(),
            ..ProgramShortcut::default()
        },
        ProgramShortcut {
            file_path: "second.lnk".into(),
            arguments: "/open:file.docx".into(),
            working_directory: "second-dir".into(),
            ..ProgramShortcut::default()
        },
        ProgramShortcut {
            file_path: "third.url".into(),
            ..ProgramShortcut::default()
        },
        ProgramShortcut {
            file_path: "shell:AppsFolder\\Contoso.Office!Word".into(),
            is_windows_app: true,
            ..ProgramShortcut::default()
        },
    ];
    let plans = LaunchPlanner::new(RecordingResolver)
        .plan(&category, LaunchTrigger::CtrlEnter)
        .expect("plan all");
    assert_eq!(
        plans.iter().map(|plan| &plan.target).collect::<Vec<_>>(),
        vec![
            &ResolvedTarget::Path {
                path: "first.exe".into(),
                kind: TargetKind::Executable
            },
            &ResolvedTarget::Path {
                path: "second.lnk".into(),
                kind: TargetKind::ShellLink
            },
            &ResolvedTarget::Path {
                path: "third.url".into(),
                kind: TargetKind::UrlShortcut
            },
            &ResolvedTarget::Path {
                path: "shell:AppsFolder\\Contoso.Office!Word".into(),
                kind: TargetKind::Other
            },
        ]
    );
    assert_eq!(plans[0].arguments, "--one \"two words\"");
    assert_eq!(plans[0].working_directory, " first-dir ");
    assert_eq!(
        plans[3].target,
        ResolvedTarget::Path {
            path: "shell:AppsFolder\\Contoso.Office!Word".into(),
            kind: TargetKind::Other
        }
    );
}

#[test]
fn planner_reports_missing_selection_without_launching() {
    let category = Category::new("Empty");
    assert_eq!(
        LaunchPlanner::new(PassthroughResolver)
            .plan(
                &category,
                LaunchTrigger::Number(ShortcutNumber::OneBased(1))
            )
            .unwrap_err(),
        PlanError::NoShortcut { index: 0 }
    );
    let mut invalid = Category::new("Invalid");
    invalid.shortcut_list.push(shortcut("   "));
    assert!(matches!(
        LaunchPlanner::new(PassthroughResolver)
            .plan(&invalid, LaunchTrigger::Number(ShortcutNumber::OneBased(1))),
        Err(PlanError::Resolve {
            source: ResolveError::EmptyPath,
            ..
        })
    ));
}

#[derive(Debug, Default)]
struct FakeExtractor {
    calls: Rc<RefCell<Vec<IconSource>>>,
}

impl IconExtractor for FakeExtractor {
    fn extract(&self, source: &IconSource, size: u32) -> Result<Vec<u8>, IconCacheError> {
        self.calls.borrow_mut().push(source.clone());
        Ok(format!("fake-{size}").into_bytes())
    }
}

#[test]
fn icon_adapter_covers_exe_link_folder_store_and_missing_targets() {
    let root = TempRoot::new("icons");
    let exe = root.0.join("app.exe");
    let link = root.0.join("app.lnk");
    let folder = root.0.join("folder");
    fs::write(&exe, b"not a real executable").expect("write exe fixture");
    fs::write(&link, b"not a real link").expect("write link fixture");
    fs::create_dir(&folder).expect("write folder fixture");
    let policy = CachePolicy::new(root.0.join("cache"), 32).unwrap();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let extractor = FakeExtractor {
        calls: calls.clone(),
    };
    for source in [
        IconSource::Executable(exe),
        IconSource::Shortcut(link),
        IconSource::Folder(folder),
        IconSource::WindowsApp {
            app_user_model_id: "Contoso.App!Main".into(),
        },
    ] {
        let entry =
            cache_icon(&policy, &extractor, "Compatibility", &source).expect("cache fake icon");
        assert_eq!(fs::read(entry.path).unwrap(), b"fake-32");
    }
    let missing = IconSource::Executable(root.0.join("missing.exe"));
    assert!(matches!(
        cache_icon(&policy, &extractor, "Compatibility", &missing),
        Err(IconCacheError::MissingTarget(_))
    ));
    assert_eq!(calls.borrow().len(), 4);
}

#[test]
fn store_discovery_fake_adapter_resolves_aumid_and_missing_apps() {
    let app = WindowsApp::new("Office", "Contoso.Office_123!Word").unwrap();
    let discovery = FakeWindowsAppDiscovery::new(vec![app.clone()]);
    assert_eq!(discovery.enumerate().unwrap(), vec![app.clone()]);
    assert_eq!(
        discovery.resolve_aumid("contoso.office_123!word").unwrap(),
        app
    );
    assert!(matches!(
        discovery.resolve_aumid("Missing.App!Main"),
        Err(WindowsAppError::NotFound { .. })
    ));
    assert!(WindowsApp::new("", "Contoso.App!Main").is_err());
}

#[test]
fn persistence_round_trip_retains_target_matrix_and_order() {
    let root = TempRoot::new("persistence");
    let paths = AppPaths::from_root(&root.0);
    let mut category = Category::new("Office Targets");
    category.allow_open_all = true;
    category.shortcut_list = vec![
        shortcut("a.exe"),
        shortcut("b.lnk"),
        shortcut("c.url"),
        shortcut("https://example.test"),
    ];
    let saved = paths.save_group(&category).expect("save group");
    assert!(saved.object_data.is_file());
    assert_eq!(paths.load_group("Office Targets").unwrap(), category);
    assert_eq!(
        saved.shortcut_path(&paths),
        root.0.join("Shortcuts/Office Targets.lnk")
    );
}

#[test]
fn shell_link_dry_run_validates_arguments_directory_identity_and_default_policy() {
    let request = ShellLinkRequest {
        link_path: PathBuf::from("Compatibility.lnk"),
        target: PathBuf::from("office.exe"),
        arguments: "--open \"file.docx\"".into(),
        working_directory: Some(PathBuf::from("C:\\Office")),
        icon_path: Some(PathBuf::from("office.exe")),
        icon_index: 0,
        description: "Office targets".into(),
        app_user_model_id: AppUserModelId::legacy_group("Office Targets").unwrap(),
        dry_run: true,
    };
    request.validate().unwrap();
    let result = create_or_update(&request).unwrap();
    assert!(!result.written);
    assert_eq!(
        result.app_user_model_id,
        "tjackenpacken.taskbarGroup.menu.Office Targets"
    );
    assert!(!PathBuf::from("Compatibility.lnk").exists());
    assert!(!ShellLinkOptions::default().run_as_user);
}

#[test]
fn launch_request_and_open_all_have_deterministic_ordering_contracts() {
    let request = LaunchRequest::from_args(["taskbar-groups.exe", "Office Targets", "ignored"]);
    assert_eq!(request.group_name.as_deref(), Some("Office Targets"));
    let mut category = Category::new("Order");
    category.shortcut_list = vec![shortcut("one.exe"), shortcut("two.exe")];
    assert!(LaunchPlanner::new(PassthroughResolver)
        .plan(&category, LaunchTrigger::CtrlEnter)
        .unwrap()
        .is_empty());
    category.allow_open_all = true;
    let plans = LaunchPlanner::new(PassthroughResolver)
        .plan(&category, LaunchTrigger::CtrlEnter)
        .unwrap();
    assert_eq!(
        plans[0].target,
        ResolvedTarget::Path {
            path: "one.exe".into(),
            kind: TargetKind::Executable
        }
    );
    assert_eq!(
        plans[1].target,
        ResolvedTarget::Path {
            path: "two.exe".into(),
            kind: TargetKind::Executable
        }
    );
}

#[cfg(windows)]
mod windows_smoke {
    use super::*;
    use taskbar_groups::platform::{LaunchError, Launcher, WindowsPlatform};

    fn system_root() -> PathBuf {
        std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
    }

    #[test]
    fn cmd_exit_launch_is_harmless_or_skipped() {
        let command = system_root().join("System32").join("cmd.exe");
        if !command.is_file() {
            eprintln!("SKIP: cmd.exe unavailable");
            return;
        }
        let spec = LaunchSpec {
            target: ResolvedTarget::Path {
                path: command.to_string_lossy().into_owned(),
                kind: TargetKind::Executable,
            },
            arguments: "/c exit 0".into(),
            working_directory: system_root().to_string_lossy().into_owned(),
        };
        match WindowsPlatform.launch(&spec) {
            Ok(()) => {}
            Err(LaunchError::Process { .. }) => eprintln!("SKIP: process launch unavailable"),
            Err(error) => panic!("unexpected harmless launch error: {error}"),
        }
    }

    #[test]
    fn shell_link_creation_is_harmless_or_skipped() {
        let target = system_root().join("System32").join("cmd.exe");
        if !target.is_file() {
            eprintln!("SKIP: cmd.exe unavailable");
            return;
        }
        let link =
            std::env::temp_dir().join(format!("taskbar-groups-target-{}.lnk", std::process::id()));
        let mut request =
            ShellLinkRequest::for_group("Target Compatibility", &link, &target).unwrap();
        request.arguments = "/c exit 0".into();
        request.working_directory = Some(system_root());
        match create_or_update(&request) {
            Ok(result) => {
                assert!(result.written && link.is_file());
                let _ = fs::remove_file(link);
            }
            Err(
                taskbar_groups::platform::shell_link::ShellLinkError::Com { .. }
                | taskbar_groups::platform::shell_link::ShellLinkError::Io(_),
            ) => eprintln!("SKIP: shell-link COM/filesystem unavailable"),
            Err(error) => panic!("unexpected shell-link error: {error}"),
        }
    }
}
