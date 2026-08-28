//! Windows Store/UWP/MSIX app discovery and AppUserModelID resolution.
//!
//! The data model and validation are portable. On Windows, discovery deliberately
//! uses the supported `Get-StartApps` shell surface instead of reading the protected
//! `WindowsApps` directory. This keeps package installation permissions out of the
//! selection path and makes an unavailable Store/Start menu facility explicit.

use std::{error::Error, fmt};

const APPS_FOLDER_PREFIX: &str = "shell:AppsFolder\\";

/// A launchable installed Windows app as exposed by the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsApp {
    pub display_name: String,
    pub aumid: String,
    pub launch_target: String,
    pub icon_candidates: Vec<String>,
    pub package: PackageMetadata,
}

/// Package information associated with a discovered app. Shell discovery may not
/// expose every field, so absent values are represented by `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageMetadata {
    pub package_name: Option<String>,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub install_location: Option<String>,
}

impl WindowsApp {
    pub fn new(
        display_name: impl Into<String>,
        aumid: impl Into<String>,
    ) -> Result<Self, WindowsAppError> {
        let display_name = display_name.into();
        let aumid = aumid.into();
        let aumid = validate_aumid(&aumid)?.to_owned();
        if display_name.trim().is_empty() {
            return Err(WindowsAppError::InvalidDisplayName);
        }
        Ok(Self {
            display_name,
            launch_target: apps_folder_target(&aumid),
            icon_candidates: vec![apps_folder_target(&aumid), aumid.clone()],
            aumid,
            package: PackageMetadata::default(),
        })
    }

    pub fn with_package(mut self, package: PackageMetadata) -> Self {
        self.package = package;
        self
    }
}

/// Validate an AUMID suitable for `shell:AppsFolder\\<AUMID>` activation.
pub fn validate_aumid(value: &str) -> Result<&str, WindowsAppError> {
    if value.is_empty() {
        return Err(WindowsAppError::InvalidAumid("AUMID is empty"));
    }
    if value.len() > 128 {
        return Err(WindowsAppError::InvalidAumid("AUMID exceeds 128 bytes"));
    }
    if value
        .chars()
        .any(|character| character.is_control() || character == '\0')
    {
        return Err(WindowsAppError::InvalidAumid(
            "AUMID contains a control character",
        ));
    }
    if value.contains(['/', '\\']) || value.chars().any(char::is_whitespace) {
        return Err(WindowsAppError::InvalidAumid(
            "AUMID contains whitespace or a path separator",
        ));
    }
    Ok(value)
}

/// Build the shell namespace target after validating the identity.
pub fn apps_folder_target(aumid: &str) -> String {
    // Callers that need error reporting should call `resolve_apps_folder_target`.
    format!("{APPS_FOLDER_PREFIX}{aumid}")
}

pub fn resolve_apps_folder_target(value: &str) -> Result<String, WindowsAppError> {
    let aumid = value.strip_prefix(APPS_FOLDER_PREFIX).unwrap_or(value);
    Ok(apps_folder_target(validate_aumid(aumid)?))
}

/// Platform boundary for app enumeration and identity resolution.
pub trait WindowsAppDiscovery {
    fn enumerate(&self) -> Result<Vec<WindowsApp>, WindowsAppError>;
    fn resolve_aumid(&self, value: &str) -> Result<WindowsApp, WindowsAppError>;
}

/// Deterministic implementation for callers and unit tests that do not need Windows.
#[derive(Debug, Clone, Default)]
pub struct FakeWindowsAppDiscovery {
    apps: Vec<WindowsApp>,
}

impl FakeWindowsAppDiscovery {
    pub fn new(apps: Vec<WindowsApp>) -> Self {
        Self { apps }
    }
}

impl WindowsAppDiscovery for FakeWindowsAppDiscovery {
    fn enumerate(&self) -> Result<Vec<WindowsApp>, WindowsAppError> {
        Ok(self.apps.clone())
    }

    fn resolve_aumid(&self, value: &str) -> Result<WindowsApp, WindowsAppError> {
        let aumid = validate_aumid(value)?;
        self.apps
            .iter()
            .find(|app| app.aumid.eq_ignore_ascii_case(aumid))
            .cloned()
            .ok_or_else(|| WindowsAppError::NotFound {
                aumid: aumid.to_owned(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsAppError {
    InvalidAumid(&'static str),
    InvalidDisplayName,
    NotFound {
        aumid: String,
    },
    Unavailable {
        operation: &'static str,
        message: String,
    },
    Command {
        operation: &'static str,
        message: String,
    },
    Parse {
        line: String,
        message: &'static str,
    },
}

impl fmt::Display for WindowsAppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAumid(message) => write!(f, "invalid Windows AppUserModelID: {message}"),
            Self::InvalidDisplayName => f.write_str("Windows app display name is empty"),
            Self::NotFound { aumid } => write!(f, "Windows app AUMID was not found: {aumid}"),
            Self::Unavailable { operation, message } => {
                write!(f, "Windows {operation} unavailable: {message}")
            }
            Self::Command { operation, message } => {
                write!(f, "Windows {operation} failed: {message}")
            }
            Self::Parse { line, message } => {
                write!(f, "could not parse Windows app row {line:?}: {message}")
            }
        }
    }
}

impl Error for WindowsAppError {}

#[cfg(windows)]
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsShellAppDiscovery;

#[cfg(windows)]
impl WindowsAppDiscovery for WindowsShellAppDiscovery {
    fn enumerate(&self) -> Result<Vec<WindowsApp>, WindowsAppError> {
        use std::process::Command;

        let output = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-StartApps | ForEach-Object { \"$($_.Name)`t$($_.AppID)\" }",
            ])
            .output()
            .map_err(|error| WindowsAppError::Unavailable {
                operation: "Start app discovery",
                message: error.to_string(),
            })?;
        if !output.status.success() {
            return Err(WindowsAppError::Command {
                operation: "Start app discovery",
                message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        parse_start_apps(&String::from_utf8_lossy(&output.stdout))
    }

    fn resolve_aumid(&self, value: &str) -> Result<WindowsApp, WindowsAppError> {
        let aumid = validate_aumid(value)?;
        self.enumerate()?
            .into_iter()
            .find(|app| app.aumid.eq_ignore_ascii_case(aumid))
            .ok_or_else(|| WindowsAppError::NotFound {
                aumid: aumid.to_owned(),
            })
    }
}

fn parse_start_apps(input: &str) -> Result<Vec<WindowsApp>, WindowsAppError> {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (_, aumid) = line
                .split_once('\t')
                .ok_or_else(|| WindowsAppError::Parse {
                    line: line.to_owned(),
                    message: "expected display name and AUMID separated by a tab",
                })?;
            if validate_aumid(aumid.trim()).is_err() {
                // Get-StartApps includes classic desktop shortcuts. Their AppID is
                // commonly an EXE path, not an AppsFolder AUMID.
                return Ok(None);
            }
            parse_start_app_row(line).map(Some)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|rows| rows.into_iter().flatten().collect())
}

fn parse_start_app_row(line: &str) -> Result<WindowsApp, WindowsAppError> {
    let (display_name, aumid) = line
        .split_once('\t')
        .ok_or_else(|| WindowsAppError::Parse {
            line: line.to_owned(),
            message: "expected display name and AUMID separated by a tab",
        })?;
    WindowsApp::new(display_name.trim(), aumid.trim()).map_err(|error| match error {
        WindowsAppError::InvalidDisplayName => WindowsAppError::Parse {
            line: line.to_owned(),
            message: "display name is empty",
        },
        WindowsAppError::InvalidAumid(_) => WindowsAppError::Parse {
            line: line.to_owned(),
            message: "AUMID is invalid",
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_normalizes_apps_folder_targets() {
        assert_eq!(
            resolve_apps_folder_target("Contoso.App_abc!App").unwrap(),
            "shell:AppsFolder\\Contoso.App_abc!App"
        );
        assert_eq!(
            resolve_apps_folder_target("shell:AppsFolder\\Contoso.App_abc!App").unwrap(),
            "shell:AppsFolder\\Contoso.App_abc!App"
        );
    }

    #[test]
    fn rejects_unsafe_aumids() {
        for value in ["", "a/b", "a\\b", "a b", "a\0b"] {
            assert!(matches!(
                validate_aumid(value),
                Err(WindowsAppError::InvalidAumid(_))
            ));
        }
    }

    #[test]
    fn parses_shell_rows_without_touching_windows_facilities() {
        let apps = parse_start_apps("Calculator\tMicrosoft.WindowsCalculator_8wekyb3d8bbwe!App\n")
            .unwrap();
        assert_eq!(apps[0].display_name, "Calculator");
        assert_eq!(
            apps[0].launch_target,
            "shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
        );
        assert_eq!(apps[0].icon_candidates.len(), 2);
    }

    #[test]
    fn fake_catalog_resolves_case_insensitively() {
        let app = WindowsApp::new("Test", "Example.App_123!App").unwrap();
        let catalog = FakeWindowsAppDiscovery::new(vec![app.clone()]);
        assert_eq!(catalog.resolve_aumid("example.app_123!app").unwrap(), app);
        assert!(matches!(
            catalog.resolve_aumid("missing!App"),
            Err(WindowsAppError::NotFound { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_discovery_skips_when_start_apps_is_unavailable() {
        match WindowsShellAppDiscovery.enumerate() {
            Ok(apps) => assert!(apps.iter().all(|app| !app.aumid.is_empty())),
            Err(WindowsAppError::Unavailable { .. } | WindowsAppError::Command { .. }) => {}
            Err(error) => panic!("unexpected discovery error: {error}"),
        }
    }
}
