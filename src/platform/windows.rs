//! Native Windows target resolution and launching.

use std::{io, os::windows::process::CommandExt, process::Command, ptr};

use super::{
    LaunchError, LaunchSpec, Launcher, PassthroughResolver, ResolveError, ResolvedTarget,
    ShortcutResolver,
};
use crate::domain::ProgramShortcut;

const SW_SHOWNORMAL: i32 = 1;

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut core::ffi::c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show_cmd: i32,
    ) -> isize;
}

/// Windows implementation of the platform launch boundary.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPlatform;

impl ShortcutResolver for WindowsPlatform {
    fn resolve(&self, shortcut: &ProgramShortcut) -> Result<ResolvedTarget, ResolveError> {
        PassthroughResolver.resolve(shortcut)
    }
}

impl Launcher for WindowsPlatform {
    fn launch(&self, spec: &LaunchSpec) -> Result<(), LaunchError> {
        match &spec.target {
            ResolvedTarget::WindowsApp { app_user_model_id } => {
                let target = format!("shell:AppsFolder\\{app_user_model_id}");
                shell_launch(&target, &spec.arguments, &spec.working_directory)
            }
            ResolvedTarget::Path { path, kind } if is_process_target(kind) => {
                process_launch(path, &spec.arguments, &spec.working_directory)
            }
            ResolvedTarget::Path { path, .. } => {
                shell_launch(path, &spec.arguments, &spec.working_directory)
            }
        }
    }
}

fn is_process_target(kind: &super::TargetKind) -> bool {
    matches!(kind, super::TargetKind::Executable)
}

fn process_launch(
    target: &str,
    arguments: &str,
    working_directory: &str,
) -> Result<(), LaunchError> {
    if target.trim().is_empty() {
        return Err(LaunchError::InvalidTarget {
            target: target.to_owned(),
            reason: "path is empty".to_owned(),
        });
    }

    let mut command = Command::new(target);
    if !arguments.is_empty() {
        // Preserve the caller's Windows command-line string. This matches the
        // legacy ProcessStartInfo behavior better than splitting it ourselves.
        command.raw_arg(arguments);
    }
    if !working_directory.trim().is_empty() {
        command.current_dir(working_directory);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| LaunchError::Process {
            target: target.to_owned(),
            message: error.to_string(),
        })
}

fn shell_launch(target: &str, arguments: &str, working_directory: &str) -> Result<(), LaunchError> {
    if target.trim().is_empty() {
        return Err(LaunchError::InvalidTarget {
            target: target.to_owned(),
            reason: "target is empty".to_owned(),
        });
    }

    let target_wide = wide(target);
    let arguments_wide = wide(arguments);
    let directory_wide = wide(working_directory);
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            ptr::null(),
            target_wide.as_ptr(),
            if arguments.is_empty() {
                ptr::null()
            } else {
                arguments_wide.as_ptr()
            },
            if working_directory.is_empty() {
                ptr::null()
            } else {
                directory_wide.as_ptr()
            },
            SW_SHOWNORMAL,
        )
    };

    if result > 32 {
        Ok(())
    } else {
        let code = if result == 0 {
            io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32
        } else {
            result as u32
        };
        let message = io::Error::from_raw_os_error(code as i32).to_string();
        Err(LaunchError::Shell {
            target: target.to_owned(),
            code,
            message,
        })
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TargetKind;
    use std::path::PathBuf;

    #[test]
    fn empty_process_target_is_reported_without_spawning() {
        let error = process_launch("", "", "").unwrap_err();
        assert!(matches!(error, LaunchError::InvalidTarget { .. }));
    }

    #[test]
    fn missing_process_is_a_structured_error() {
        let error = process_launch(
            r"C:\this-path-should-not-exist\taskbar-groups-test.exe",
            "",
            "",
        )
        .unwrap_err();
        assert!(matches!(error, LaunchError::Process { .. }));
    }

    #[test]
    fn available_command_target_launches_and_skips_when_unavailable() {
        let system_root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let command = system_root.join("System32").join("cmd.exe");
        if !command.is_file() {
            return;
        }

        let spec = LaunchSpec {
            target: ResolvedTarget::Path {
                path: command.to_string_lossy().into_owned(),
                kind: TargetKind::Executable,
            },
            arguments: "/c exit 0".to_owned(),
            working_directory: system_root.to_string_lossy().into_owned(),
        };
        WindowsPlatform.launch(&spec).unwrap();
    }

    #[test]
    fn store_targets_use_apps_folder_shell_namespace() {
        let target = format!("shell:AppsFolder\\{}", "example.app!App");
        assert!(target.starts_with("shell:AppsFolder\\"));
    }
}
