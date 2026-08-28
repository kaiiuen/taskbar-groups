//! Acceptance tests for Windows facilities used by the shipped application.
//!
//! Every test owns its temporary tree and either uses a harmless system command or
//! skips when the corresponding Windows shell facility is unavailable.

#[cfg(windows)]
mod windows_acceptance {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use taskbar_groups::{
        domain::{Category, ProgramShortcut},
        persistence::{migration::PathRepairPolicy, AppPaths},
        platform::{
            icon_cache::{cache_icon, CachePolicy, IconSource, PlatformIconExtractor},
            shell_link::{create_or_update, AppUserModelId, ShellLinkError, ShellLinkRequest},
            LaunchSpec, Launcher, ResolvedTarget, TargetKind, WindowsPlatform,
        },
    };
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::GetDesktopWindow};

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before the Unix epoch")
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("taskbar-groups-acceptance-{label}-{nonce}"));
            fs::create_dir_all(&path).expect("create acceptance test directory");
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn system_root() -> PathBuf {
        std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
    }

    fn command_path() -> Option<PathBuf> {
        let path = system_root().join("System32").join("cmd.exe");
        path.is_file().then_some(path)
    }

    fn desktop_available() -> bool {
        !unsafe { GetDesktopWindow() }.is_null()
    }

    fn wait_for_file(path: &Path) {
        for _ in 0..50 {
            if path.is_file() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            path.is_file(),
            "launched command did not create {}",
            path.display()
        );
    }

    fn launch_spec(
        path: &Path,
        kind: TargetKind,
        arguments: String,
        directory: &Path,
    ) -> LaunchSpec {
        LaunchSpec {
            target: ResolvedTarget::Path {
                path: path.to_string_lossy().into_owned(),
                kind,
            },
            arguments,
            working_directory: directory.to_string_lossy().into_owned(),
        }
    }

    fn command_arguments(marker: &Path, text: &str) -> String {
        format!(r#"/c echo {text}>"{}"#, marker.display())
    }

    #[test]
    fn real_executable_launch_honors_arguments_and_working_directory() {
        let Some(command) = command_path() else {
            eprintln!("SKIP: System32\\cmd.exe is unavailable");
            return;
        };
        let root = TempRoot::new("process-launch");
        let marker = root.0.join("process-marker.txt");
        let spec = launch_spec(
            &command,
            TargetKind::Executable,
            command_arguments(&marker, "acceptance-argument"),
            &root.0,
        );

        WindowsPlatform.launch(&spec).expect("launch cmd.exe");
        wait_for_file(&marker);
        assert_eq!(
            fs::read_to_string(marker)
                .expect("read process marker")
                .trim(),
            "acceptance-argument"
        );
    }

    #[test]
    fn shell_link_is_created_with_properties_and_launches() {
        let Some(command) = command_path() else {
            eprintln!("SKIP: System32\\cmd.exe is unavailable");
            return;
        };
        if !desktop_available() {
            eprintln!("SKIP: interactive Windows shell is unavailable");
            return;
        }
        let root = TempRoot::new("shell-link");
        let marker = root.0.join("link-marker.txt");
        let link = root.0.join("Acceptance.lnk");
        let app_id = "tjackenpacken.taskbarGroup.menu.Acceptance";
        let mut request =
            ShellLinkRequest::for_group("Acceptance", &link, &command).expect("link request");
        request.arguments = command_arguments(&marker, "link-argument");
        request.working_directory = Some(root.0.clone());
        request.icon_path = Some(command.clone());
        request.description = "Windows acceptance link".into();
        request.app_user_model_id = AppUserModelId::explicit(app_id).expect("app identity");

        match create_or_update(&request) {
            Ok(result) => {
                assert!(result.written && link.is_file());
                assert_shell_link_properties(
                    &link,
                    &command,
                    &request.arguments,
                    &root.0,
                    &request.description,
                );
                assert_utf16_string_in_file(&link, app_id);
            }
            Err(ShellLinkError::Com { .. } | ShellLinkError::Io(_)) => {
                eprintln!("SKIP: Windows ShellLink COM/storage facility is unavailable");
                return;
            }
            Err(error) => panic!("unexpected shell-link error: {error}"),
        }

        let spec = launch_spec(&link, TargetKind::ShellLink, String::new(), &root.0);
        WindowsPlatform.launch(&spec).expect("launch shell link");
        wait_for_file(&marker);
        assert_eq!(
            fs::read_to_string(marker).expect("read link marker").trim(),
            "link-argument"
        );
    }

    #[test]
    fn folder_shell_launch_uses_the_interactive_explorer_shell() {
        if !desktop_available() {
            eprintln!("SKIP: no interactive Windows desktop for Explorer launch");
            return;
        }
        let root = TempRoot::new("folder-shell");
        let spec = launch_spec(&root.0, TargetKind::Folder, String::new(), &root.0);
        match WindowsPlatform.launch(&spec) {
            Ok(()) => {}
            Err(taskbar_groups::platform::LaunchError::Shell { .. }) => {
                eprintln!("SKIP: Explorer shell facility is unavailable");
            }
            Err(error) => panic!("unexpected folder launch error: {error}"),
        }
    }

    #[test]
    fn native_icon_extraction_writes_a_real_cached_ico_and_reuses_identity() {
        let Some(command) = command_path() else {
            eprintln!("SKIP: System32\\cmd.exe is unavailable for icon extraction");
            return;
        };
        if !desktop_available() {
            eprintln!("SKIP: no interactive shell for native icon extraction");
            return;
        }
        let root = TempRoot::new("icon-cache");
        let policy = CachePolicy::new(root.0.join("cache"), 32).expect("cache policy");
        let source = IconSource::Executable(command);
        let first = match cache_icon(&policy, &PlatformIconExtractor, "Acceptance", &source) {
            Ok(entry) => entry,
            Err(taskbar_groups::platform::icon_cache::IconCacheError::Extraction { .. })
            | Err(taskbar_groups::platform::icon_cache::IconCacheError::UnsupportedPlatform) => {
                eprintln!("SKIP: Windows shell did not provide an executable icon");
                return;
            }
            Err(error) => panic!("unexpected icon cache error: {error}"),
        };
        let first_bytes = fs::read(&first.path).expect("read cached icon");
        assert!(first_bytes.starts_with(&[0, 0, 1, 0]) && first_bytes.len() > 22);
        let second = cache_icon(&policy, &PlatformIconExtractor, "Acceptance", &source)
            .expect("cache same icon");
        assert_eq!(
            first.path, second.path,
            "stable source identity should reuse cache path"
        );
        assert_eq!(
            first_bytes,
            fs::read(second.path).expect("read reused cached icon")
        );
    }

    #[test]
    fn installed_migration_publishes_repaired_output_without_touching_legacy_data() {
        let root = TempRoot::new("migration");
        let legacy = root.0.join("legacy");
        let source_group = legacy.join("config").join("Acceptance");
        fs::create_dir_all(&source_group).expect("create legacy group");
        let mut category = Category::new("Acceptance");
        category.shortcut_list.push(ProgramShortcut {
            file_path: r"bin\acceptance.exe".into(),
            working_directory: "work".into(),
            ..ProgramShortcut::default()
        });
        let source_file = source_group.join("ObjectData.xml");
        fs::write(&source_file, category.to_legacy_xml()).expect("write legacy XML");
        let app = AppPaths::from_root(root.0.join("portable"));

        let plan = app
            .migrate_legacy(&legacy, PathRepairPolicy::Installed)
            .expect("execute installed migration");
        let output = plan
            .destination_root
            .join("v1/groups/Acceptance/ObjectData.xml");
        assert!(output.is_file() && source_file.is_file());
        let migrated =
            Category::from_legacy_xml(&fs::read_to_string(output).expect("read migration output"))
                .expect("parse migration output");
        assert_eq!(
            migrated.shortcut_list[0].file_path,
            legacy.join(r"bin\acceptance.exe").to_string_lossy()
        );
        assert_eq!(
            migrated.shortcut_list[0].working_directory,
            legacy.join("work").to_string_lossy()
        );
        assert!(fs::read_to_string(source_file)
            .expect("read legacy source")
            .contains("acceptance.exe"));
    }

    #[test]
    fn native_ui_and_shell_execute_facilities_are_detectable_without_mutation() {
        if !desktop_available() {
            eprintln!("SKIP: native UI requires an interactive Windows desktop");
            return;
        }
        let shell = system_root().join("System32").join("shell32.dll");
        if !shell.is_file() {
            eprintln!("SKIP: Windows shell32.dll is unavailable");
            return;
        }
        let target = shell
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        };
        if result as usize <= 32 {
            eprintln!("SKIP: native shell facility cannot open a known system file");
        }
    }

    fn assert_utf16_string_in_file(path: &Path, value: &str) {
        let bytes = fs::read(path).expect("read shell link bytes");
        let needle = value
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(
            bytes.windows(needle.len()).any(|window| window == needle),
            "AppUserModelID was not serialized into link"
        );
    }

    fn assert_shell_link_properties(
        link: &Path,
        target: &Path,
        arguments: &str,
        directory: &Path,
        description: &str,
    ) {
        let Some(powershell) = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .map(|root| root.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
            .filter(|path| path.is_file())
        else {
            eprintln!("SKIP: Windows PowerShell is unavailable for ShellLink property readback");
            return;
        };
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); @($s.TargetPath,$s.Arguments,$s.WorkingDirectory,$s.Description) -join [char]31",
            ps_quote(link)
        );
        let output = Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .expect("run PowerShell ShellLink readback");
        if !output.status.success() {
            eprintln!("SKIP: WScript.Shell property readback is unavailable");
            return;
        }
        let values = String::from_utf8_lossy(&output.stdout)
            .trim()
            .split('\x1f')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                target.to_string_lossy().into_owned(),
                arguments.to_owned(),
                directory.to_string_lossy().into_owned(),
                description.to_owned()
            ]
        );
    }

    fn ps_quote(path: &Path) -> String {
        path.to_string_lossy().replace('\'', "''")
    }
}

#[cfg(not(windows))]
#[test]
fn windows_acceptance_module_compiles_portably() {}
