//! Classification and launch planning for URI and other special targets.
//!
//! This module deliberately keeps target interpretation separate from persistence and
//! UI. Planning is portable; the Windows executor is compiled only on Windows.

use std::fmt;

/// The target families that need shell-aware handling rather than executable spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecialTargetKind {
    SteamGame { app_id: String },
    BrowserUrl,
    PwaUrl,
    ShellUri { scheme: String },
    UrlShortcut,
}

/// Elevation is opt-in. Normal launches never request a UAC prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ElevationPolicy {
    #[default]
    Never,
    RunAs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecialTargetPlan {
    pub kind: SpecialTargetKind,
    pub target: String,
    pub arguments: String,
    pub working_directory: String,
    pub elevation: ElevationPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecialTargetError {
    EmptyTarget,
    EmbeddedNul {
        field: &'static str,
    },
    InvalidSteamUri {
        target: String,
    },
    InvalidUri {
        target: String,
        reason: &'static str,
    },
}

impl fmt::Display for SpecialTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTarget => f.write_str("special target is empty"),
            Self::EmbeddedNul { field } => write!(f, "{field} contains an embedded NUL"),
            Self::InvalidSteamUri { target } => write!(f, "invalid Steam URI: {target}"),
            Self::InvalidUri { target, reason } => write!(f, "invalid URI {target}: {reason}"),
        }
    }
}

impl std::error::Error for SpecialTargetError {}

/// Classify and normalize a shell-oriented target without starting anything.
pub fn plan(
    target: &str,
    arguments: &str,
    working_directory: &str,
    elevation: ElevationPolicy,
) -> Result<Option<SpecialTargetPlan>, SpecialTargetError> {
    if target.contains('\0') {
        return Err(SpecialTargetError::EmbeddedNul { field: "target" });
    }
    if arguments.contains('\0') {
        return Err(SpecialTargetError::EmbeddedNul { field: "arguments" });
    }
    if working_directory.contains('\0') {
        return Err(SpecialTargetError::EmbeddedNul {
            field: "working directory",
        });
    }

    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err(SpecialTargetError::EmptyTarget);
    }

    let lower = trimmed.to_ascii_lowercase();
    let kind = if lower.ends_with(".url") {
        Some(SpecialTargetKind::UrlShortcut)
    } else if lower.starts_with("steam://") {
        Some(SpecialTargetKind::SteamGame {
            app_id: steam_app_id(trimmed)?,
        })
    } else if let Some(scheme) = uri_scheme(trimmed) {
        Some(match scheme.as_str() {
            "http" | "https" => SpecialTargetKind::BrowserUrl,
            "ms-edge" | "microsoft-edge" | "chrome" | "brave" | "opera" => {
                SpecialTargetKind::PwaUrl
            }
            _ => SpecialTargetKind::ShellUri { scheme },
        })
    } else {
        None
    };

    let normalized_target = if lower.starts_with("steam://") {
        format!(
            "steam://{}",
            trimmed.split_once("://").unwrap().1.to_ascii_lowercase()
        )
    } else {
        trimmed.to_owned()
    };

    Ok(kind.map(|kind| SpecialTargetPlan {
        kind,
        target: normalized_target,
        arguments: arguments.to_owned(),
        working_directory: working_directory.trim().to_owned(),
        elevation,
    }))
}

fn steam_app_id(target: &str) -> Result<String, SpecialTargetError> {
    let Some((scheme, rest)) = target.split_once("://") else {
        return Err(SpecialTargetError::InvalidSteamUri {
            target: target.to_owned(),
        });
    };
    if !scheme.eq_ignore_ascii_case("steam") {
        return Err(SpecialTargetError::InvalidSteamUri {
            target: target.to_owned(),
        });
    }
    let rest = rest.to_ascii_lowercase();
    let id = rest
        .strip_prefix("rungameid/")
        .or_else(|| rest.strip_prefix("run/"))
        .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()));
    id.map(str::to_owned)
        .ok_or_else(|| SpecialTargetError::InvalidSteamUri {
            target: target.to_owned(),
        })
}

fn uri_scheme(target: &str) -> Option<String> {
    let (scheme, remainder) = target.split_once(':')?;
    if scheme.is_empty()
        || !scheme.chars().enumerate().all(|(index, c)| {
            (index == 0 && c.is_ascii_alphabetic())
                || (index > 0 && (c.is_ascii_alphanumeric() || "+-.".contains(c)))
        })
        || remainder.is_empty()
    {
        return None;
    }
    Some(scheme.to_ascii_lowercase())
}

#[cfg(windows)]
mod windows {
    use super::{ElevationPolicy, SpecialTargetPlan};
    use crate::platform::LaunchError;
    use std::{io, ptr};

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

    pub(crate) fn launch(plan: &SpecialTargetPlan) -> Result<(), LaunchError> {
        launch_values(
            &plan.target,
            &plan.arguments,
            &plan.working_directory,
            plan.elevation,
        )
    }

    pub(crate) fn launch_target(
        target: &str,
        arguments: &str,
        working_directory: &str,
        elevation: ElevationPolicy,
    ) -> Result<(), LaunchError> {
        launch_values(target, arguments, working_directory, elevation)
    }

    fn launch_values(
        target: &str,
        arguments: &str,
        working_directory: &str,
        elevation: ElevationPolicy,
    ) -> Result<(), LaunchError> {
        let operation = match elevation {
            ElevationPolicy::Never => "open",
            ElevationPolicy::RunAs => "runas",
        };
        if target.trim().is_empty() {
            return Err(LaunchError::InvalidTarget {
                target: target.to_owned(),
                reason: "target is empty".to_owned(),
            });
        }
        let target_wide = wide(target);
        let arguments_wide = wide(arguments);
        let directory_wide = wide(working_directory);
        let operation_wide = wide(operation);
        let result = unsafe {
            ShellExecuteW(
                ptr::null_mut(),
                operation_wide.as_ptr(),
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
            Err(LaunchError::Shell {
                target: target.to_owned(),
                code,
                message: io::Error::from_raw_os_error(code as i32).to_string(),
            })
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(windows)]
pub(crate) use windows::{launch as launch_windows, launch_target as launch_windows_target};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_steam_game_uri_without_launching() {
        let result = plan(
            " STEAM://rungameid/730 ",
            "-silent",
            "C:\\Games",
            ElevationPolicy::Never,
        )
        .unwrap();
        let planned = result.unwrap();
        assert_eq!(planned.target, "steam://rungameid/730");
        assert_eq!(
            planned.kind,
            SpecialTargetKind::SteamGame {
                app_id: "730".into()
            }
        );
    }

    #[test]
    fn classifies_browser_and_pwa_urls() {
        assert_eq!(
            plan("https://example.test/app", "", "", ElevationPolicy::Never)
                .unwrap()
                .unwrap()
                .kind,
            SpecialTargetKind::BrowserUrl
        );
        assert_eq!(
            plan(
                "microsoft-edge:https://example.test",
                "",
                "",
                ElevationPolicy::Never
            )
            .unwrap()
            .unwrap()
            .kind,
            SpecialTargetKind::PwaUrl
        );
    }

    #[test]
    fn preserves_arguments_and_normalizes_working_directory() {
        let planned = plan(
            "steam://run/10",
            "--quoted=\"a b\"",
            "  C:\\Games  ",
            ElevationPolicy::Never,
        )
        .unwrap()
        .unwrap();
        assert_eq!(planned.arguments, "--quoted=\"a b\"");
        assert_eq!(planned.working_directory, "C:\\Games");
        assert_eq!(planned.elevation, ElevationPolicy::Never);
    }

    #[test]
    fn rejects_malformed_or_unsafe_inputs() {
        assert!(matches!(
            plan("steam://rungameid/nope", "", "", ElevationPolicy::Never),
            Err(SpecialTargetError::InvalidSteamUri { .. })
        ));
        assert!(matches!(
            plan("https://example.test\0", "", "", ElevationPolicy::Never),
            Err(SpecialTargetError::EmbeddedNul { field: "target" })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn elevation_is_explicit_in_a_safe_plan() {
        let planned = plan("https://example.test", "", "", ElevationPolicy::RunAs)
            .unwrap()
            .unwrap();
        assert_eq!(planned.elevation, ElevationPolicy::RunAs);
    }
}
