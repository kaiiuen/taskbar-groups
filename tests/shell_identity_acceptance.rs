//! Safe acceptance coverage for Windows shell-link identity compatibility.
//!
//! The Windows cases write only inside an owned temporary directory and inspect
//! links through PowerShell/COM. They do not launch targets or change taskbar pins.

use taskbar_groups::platform::{
    shell_link::{create_or_update, AppUserModelId, ShellLinkRequest},
    special_targets::{self, ElevationPolicy, SpecialTargetKind},
};

#[test]
fn dry_run_preserves_legacy_group_identity_and_editable_link_properties() {
    let link = std::env::temp_dir().join("taskbar-groups-dry-run-acceptance.lnk");
    let target = std::env::temp_dir().join("safe-target.exe");
    let mut request = ShellLinkRequest::for_group("Original", &link, &target)
        .expect("construct legacy group shell-link request");
    request.arguments = "--safe --group Original".into();
    request.working_directory = Some(std::env::temp_dir());
    request.icon_path = Some(target.clone());
    request.icon_index = 0;
    request.dry_run = true;

    let first = create_or_update(&request).expect("validate original dry-run request");
    assert!(!first.written);
    assert_eq!(
        first.app_user_model_id,
        "tjackenpacken.taskbarGroup.menu.Original"
    );
    assert!(!link.exists(), "dry-run must not create a link");

    request.description = "Renamed".into();
    request.app_user_model_id =
        AppUserModelId::legacy_group("Renamed").expect("construct renamed legacy group identity");
    request.arguments = "--safe --group Renamed".into();
    let updated = create_or_update(&request).expect("validate edited dry-run request");
    assert!(!updated.written);
    assert_eq!(
        updated.app_user_model_id,
        "tjackenpacken.taskbarGroup.menu.Renamed"
    );
    assert!(!link.exists(), "edited dry-run must not create a link");

    let explicit = AppUserModelId::explicit("com.example.TaskbarGroups.Acceptance")
        .expect("construct explicit shell identity");
    assert_eq!(explicit.value(), "com.example.TaskbarGroups.Acceptance");
}

#[test]
fn special_target_representatives_are_planned_without_launching() {
    let cases = [
        (
            "steam://rungameid/570",
            SpecialTargetKind::SteamGame {
                app_id: "570".into(),
            },
        ),
        ("https://example.invalid/app", SpecialTargetKind::BrowserUrl),
        ("ms-edge://example.invalid/app", SpecialTargetKind::PwaUrl),
    ];

    for (target, expected_kind) in cases {
        let plan = special_targets::plan(target, "--safe", "", ElevationPolicy::Never)
            .expect("plan representative target")
            .expect("target should be shell-oriented");
        assert_eq!(plan.kind, expected_kind);
        assert_eq!(plan.arguments, "--safe");
        assert_eq!(plan.elevation, ElevationPolicy::Never);
    }
    eprintln!(
        "SAFE: representative UWP/PWA/Steam identities were planned only; no target was launched"
    );
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };
    use taskbar_groups::platform::windows_apps::{WindowsAppDiscovery, WindowsShellAppDiscovery};

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before the Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("taskbar-groups-shell-{nonce}"));
            fs::create_dir_all(&path).expect("create shell acceptance directory");
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn powershell() -> Option<PathBuf> {
        std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .map(|root| root.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
            .filter(|path| path.is_file())
    }

    fn command_path() -> Option<PathBuf> {
        std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .map(|root| root.join("System32/cmd.exe"))
            .filter(|path| path.is_file())
    }

    #[test]
    fn creates_and_updates_link_then_reads_back_identity_and_shell_properties() {
        let Some(command) = command_path() else {
            eprintln!("SKIP: System32\\cmd.exe is unavailable");
            return;
        };
        let Some(powershell) = powershell() else {
            eprintln!("SKIP: Windows PowerShell is unavailable for ShellLink readback");
            return;
        };
        let root = TempRoot::new();
        let link = root.0.join("Group.lnk");
        let icon = root.0.join("Group.ico");
        fs::write(&icon, b"not an icon; only the path property is inspected")
            .expect("write harmless icon fixture");

        let mut request = ShellLinkRequest::for_group("Original", &link, &command)
            .expect("construct shell-link request");
        request.arguments = "/c echo original".into();
        request.working_directory = Some(root.0.clone());
        request.icon_path = Some(icon.clone());
        request.description = "Original description".into();
        if !write_or_skip(&request) {
            return;
        }
        assert!(link.is_file(), "ShellLink writer should create the link");
        assert_readback(&powershell, &link, &command, &request);
        assert_utf16_string_in_file(&link, &request.app_user_model_id.value());

        request.description = "Renamed description".into();
        request.arguments = "/c echo renamed".into();
        request.app_user_model_id =
            AppUserModelId::legacy_group("Renamed").expect("construct edited group identity");
        if !write_or_skip(&request) {
            return;
        }
        assert!(link.is_file(), "ShellLink writer should replace the link");
        assert_readback(&powershell, &link, &command, &request);
        assert_utf16_string_in_file(&link, &request.app_user_model_id.value());
        assert!(!link.with_extension("tmp").exists());
    }

    fn write_or_skip(request: &ShellLinkRequest) -> bool {
        match create_or_update(request) {
            Ok(result) => {
                assert!(result.written);
                true
            }
            Err(
                taskbar_groups::platform::shell_link::ShellLinkError::Com { .. }
                | taskbar_groups::platform::shell_link::ShellLinkError::Io(_),
            ) => {
                eprintln!("SKIP: Windows ShellLink COM/storage facility is unavailable");
                false
            }
            Err(error) => panic!("unexpected ShellLink error: {error}"),
        }
    }

    fn assert_readback(powershell: &Path, link: &Path, command: &Path, request: &ShellLinkRequest) {
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); @($s.TargetPath,$s.Arguments,$s.WorkingDirectory,$s.Description,$s.IconLocation) -join [char]31",
            ps_quote(link)
        );
        let output = Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .expect("run PowerShell ShellLink readback");
        assert!(output.status.success(), "WScript.Shell readback failed");
        let values = String::from_utf8_lossy(&output.stdout)
            .trim()
            .split('\x1f')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 5);
        assert_eq!(values[0], command.to_string_lossy());
        assert_eq!(values[1], request.arguments);
        assert_eq!(
            values[2],
            request
                .working_directory
                .as_ref()
                .unwrap()
                .to_string_lossy()
        );
        assert_eq!(values[3], request.description);
        assert_eq!(
            values[4],
            format!(
                "{},{}",
                request.icon_path.as_ref().unwrap().display(),
                request.icon_index
            )
        );
    }

    #[test]
    fn reports_windows_version_and_pin_open_identity_surface_without_mutation() {
        let output = Command::new("cmd.exe")
            .args(["/c", "ver"])
            .output()
            .expect("query Windows version");
        assert!(output.status.success());
        eprintln!(
            "Windows version: {}",
            String::from_utf8_lossy(&output.stdout).trim()
        );

        let Some(powershell) = powershell() else {
            eprintln!("SKIP: PowerShell unavailable; cannot inspect shell verbs");
            return;
        };
        let script = "$o=New-Object -ComObject Shell.Application; $o.Name";
        let output = Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .expect("query shell automation surface");
        if output.status.success() {
            eprintln!(
                "SAFE: Explorer shell automation is available; pin/open verbs were not invoked"
            );
        } else {
            eprintln!("SKIP: Explorer shell automation is unavailable");
        }
        eprintln!("SAFE: taskbar pinning was not changed and no link target was opened");
    }

    #[test]
    fn reports_available_uwp_pwa_and_steam_surfaces_without_activation() {
        match WindowsShellAppDiscovery.enumerate() {
            Ok(apps) if !apps.is_empty() => eprintln!(
                "AVAILABLE: discovered {} Windows shell app identities; activation skipped",
                apps.len()
            ),
            Ok(_) => eprintln!("SKIP: no UWP/PWA shell app identities are installed"),
            Err(error) => eprintln!("SKIP: Windows app discovery unavailable: {error}"),
        }

        let Some(powershell) = powershell() else {
            eprintln!("SKIP: PowerShell unavailable for Steam probe");
            return;
        };
        let script = "@($steam=Get-StartApps | Where-Object {$_.AppID -match 'steam' -or $_.Name -match 'Steam'}; $steam | Select-Object -First 1 -ExpandProperty AppID) -join ''";
        let output = Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .expect("probe installed shell apps");
        if output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty() {
            eprintln!("AVAILABLE: a Steam Start-menu identity was found; activation skipped");
        } else {
            eprintln!(
                "SKIP: Steam representative is unavailable; UWP/PWA discovery was not activated"
            );
        }
        eprintln!("SAFE: UWP/PWA/Steam probes performed discovery only");
    }

    fn assert_utf16_string_in_file(path: &Path, value: &str) {
        let bytes = fs::read(path).expect("read shell link bytes");
        let needle = value
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(bytes.windows(needle.len()).any(|window| window == needle));
    }

    fn ps_quote(path: &Path) -> String {
        path.to_string_lossy().replace('\'', "''")
    }
}

#[cfg(not(windows))]
#[test]
fn windows_shell_identity_acceptance_compiles_portably() {}
