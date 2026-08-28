//! Windows Store/UWP/MSIX app discovery and AppUserModelID resolution.
//!
//! The data model and validation are portable. On Windows, discovery uses the
//! supported WinRT package/AppListEntry APIs, with Start menu and AppX PowerShell
//! surfaces as fallbacks. This avoids reading the protected `WindowsApps` directory.
//! Metadata is best-effort: a usable app row is retained when package or manifest
//! details are unavailable.

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
    pub package_family_name: Option<String>,
    pub package_full_name: Option<String>,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub install_location: Option<String>,
    pub application_id: Option<String>,
    pub executable: Option<String>,
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

    fn with_metadata(mut self, package: PackageMetadata, manifest_icon: Option<String>) -> Self {
        if let Some(icon) = manifest_icon.filter(|value| !value.trim().is_empty()) {
            self.icon_candidates.insert(0, icon);
        }
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
        if let Ok(apps) = discover_winrt_apps() {
            if !apps.is_empty() {
                return Ok(apps);
            }
        }

        use std::process::Command;

        // Get-StartApps remains the shell fallback. The additional AppX queries
        // enrich each row, but never make a valid shell row unusable.
        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", METADATA_SCRIPT])
            .output()
            .map_err(|error| WindowsAppError::Unavailable {
                operation: "Store app metadata discovery",
                message: error.to_string(),
            })?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Ok(apps) = parse_metadata_rows(&text) {
                if !apps.is_empty() {
                    return Ok(apps);
                }
            }
        }

        let fallback = Command::new("powershell.exe")
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
        if !fallback.status.success() {
            return Err(WindowsAppError::Command {
                operation: "Start app discovery",
                message: String::from_utf8_lossy(&fallback.stderr).trim().to_owned(),
            });
        }
        parse_start_apps(&String::from_utf8_lossy(&fallback.stdout))
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

#[cfg(windows)]
fn discover_winrt_apps() -> Result<Vec<WindowsApp>, WindowsAppError> {
    use windows::ApplicationModel::Core::AppListEntry;
    use windows::Management::Deployment::PackageManager;

    let manager = PackageManager::new().map_err(|error| WindowsAppError::Unavailable {
        operation: "WinRT package discovery",
        message: error.to_string(),
    })?;
    let packages = manager
        .FindPackages()
        .map_err(|error| WindowsAppError::Unavailable {
            operation: "WinRT package enumeration",
            message: error.to_string(),
        })?;
    let mut apps = Vec::new();

    for package in packages
        .First()
        .map_err(|error| WindowsAppError::Unavailable {
            operation: "WinRT package enumeration",
            message: error.to_string(),
        })?
    {
        if package.IsFramework().unwrap_or(true) {
            continue;
        }
        let id = match package.Id() {
            Ok(id) => id,
            Err(_) => continue,
        };
        let package_path = package
            .InstalledLocation()
            .ok()
            .and_then(|location| location.Path().ok().map(|path| path.to_string()));
        let mut package_metadata = PackageMetadata::default();
        package_metadata.package_name = id.Name().ok().map(|value| value.to_string());
        package_metadata.package_family_name = id.FamilyName().ok().map(|value| value.to_string());
        package_metadata.package_full_name = id.FullName().ok().map(|value| value.to_string());
        package_metadata.publisher = id.Publisher().ok().map(|value| value.to_string());
        package_metadata.version = id.Version().ok().map(|version| {
            format!(
                "{}.{}.{}.{}",
                version.Major, version.Minor, version.Build, version.Revision
            )
        });
        package_metadata.install_location = package_path.clone();

        let entries = match package.GetAppListEntries() {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for index in 0..entries.Size().unwrap_or(0) {
            let entry: AppListEntry = match entries.GetAt(index) {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let aumid = match entry.AppUserModelId() {
                Ok(value) => value.to_string(),
                Err(_) => continue,
            };
            let display_name = entry
                .DisplayInfo()
                .ok()
                .and_then(|info| info.DisplayName().ok())
                .map(|value| value.to_string())
                .or_else(|| package.DisplayName().ok().map(|value| value.to_string()))
                .unwrap_or_default();
            let Ok(mut app) = WindowsApp::new(display_name, &aumid) else {
                continue;
            };
            package_metadata.application_id = aumid.split('!').nth(1).map(str::to_owned);
            let manifest = package_path
                .as_deref()
                .and_then(|path| {
                    std::fs::read_to_string(std::path::Path::new(path).join("AppxManifest.xml"))
                        .ok()
                })
                .and_then(|xml: String| {
                    parse_manifest_application(
                        xml.as_str(),
                        package_metadata.application_id.as_deref(),
                    )
                });
            if let Some(manifest) = manifest {
                if let Some(executable) = manifest.executable {
                    package_metadata.executable = package_path.as_deref().map(|path| {
                        std::path::Path::new(path)
                            .join(executable)
                            .to_string_lossy()
                            .into_owned()
                    });
                }
                app = app.with_metadata(
                    package_metadata.clone(),
                    manifest.logo.map(|logo| {
                        package_path
                            .as_deref()
                            .map(|path| {
                                std::path::Path::new(path)
                                    .join(&logo)
                                    .to_string_lossy()
                                    .into_owned()
                            })
                            .unwrap_or(logo)
                    }),
                );
            } else {
                app = app.with_package(package_metadata.clone());
            }
            apps.push(app);
        }
    }
    Ok(apps)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ManifestApplication {
    executable: Option<String>,
    logo: Option<String>,
}

fn parse_manifest_application(
    xml: &str,
    application_id: Option<&str>,
) -> Option<ManifestApplication> {
    let mut remainder = xml;
    while let Some(start) = remainder.find("<Application ") {
        let application = &remainder[start + 1..];
        let (attributes, body) = application.split_once('>')?;
        let id = xml_attribute(attributes, "Id");
        let end = body.find("</Application>").unwrap_or(0);
        let body = &body[..end];
        if application_id.is_none() || id.as_deref() == application_id {
            let visual_attributes = body
                .find("VisualElements")
                .and_then(|position| body[position..].split_once('>').map(|(tag, _)| tag));
            return Some(ManifestApplication {
                executable: xml_attribute(attributes, "Executable"),
                logo: visual_attributes.and_then(|tag| xml_attribute(tag, "Logo")),
            });
        }
        remainder = &application[attributes.len() + 1 + end..];
    }
    None
}

fn xml_attribute(attributes: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=\"");
    let value = attributes.split_once(&marker)?.1.split_once('\"')?.0;
    (!value.trim().is_empty()).then(|| value.to_owned())
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

fn parse_metadata_rows(input: &str) -> Result<Vec<WindowsApp>, WindowsAppError> {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_metadata_row)
        .collect()
}

fn decode_field(value: &str, line: &str) -> Result<String, WindowsAppError> {
    let mut bytes = Vec::with_capacity(value.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for character in value.bytes() {
        if character == b'=' {
            break;
        }
        let digit = match character {
            b'A'..=b'Z' => character - b'A',
            b'a'..=b'z' => character - b'a' + 26,
            b'0'..=b'9' => character - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'\r' | b'\n' => continue,
            _ => {
                return Err(WindowsAppError::Parse {
                    line: line.to_owned(),
                    message: "metadata field is not valid UTF-8 base64",
                })
            }
        } as u32;
        buffer = (buffer << 6) | digit;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    String::from_utf8(bytes).map_err(|_| WindowsAppError::Parse {
        line: line.to_owned(),
        message: "metadata field is not valid UTF-8 base64",
    })
}

fn parse_metadata_row(line: &str) -> Result<WindowsApp, WindowsAppError> {
    let fields = line.split('\t').collect::<Vec<_>>();
    if fields.len() != 10 {
        return Err(WindowsAppError::Parse {
            line: line.to_owned(),
            message: "expected ten base64 metadata fields",
        });
    }
    let values = fields
        .iter()
        .map(|field| decode_field(field, line))
        .collect::<Result<Vec<_>, _>>()?;
    let mut package = PackageMetadata::default();
    package.package_name = nonempty(&values[2]);
    package.package_family_name = nonempty(&values[3]);
    package.package_full_name = nonempty(&values[4]);
    package.publisher = nonempty(&values[5]);
    package.version = nonempty(&values[6]);
    package.install_location = nonempty(&values[7]);
    package.application_id = nonempty(&values[1].split('!').nth(1).unwrap_or_default());
    package.executable = nonempty(&values[8]);
    let app = WindowsApp::new(&values[0], &values[1])?;
    Ok(app.with_metadata(package, nonempty(&values[9])))
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_owned())
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

#[cfg(windows)]
const METADATA_SCRIPT: &str = r#"
function B64([object] $value) {
  [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes([string]$value))
}
Get-StartApps | ForEach-Object {
  $start = $_
  if ($start.AppID -notmatch '^[^!]+![^!]+$') { return }
  $parts = $start.AppID -split '!', 2
  $package = Get-AppxPackage -PackageTypeFilter Main | Where-Object PackageFamilyName -eq $parts[0] | Select-Object -First 1
  $application = $null
  $icon = ''
  $executable = ''
  if ($package) {
    try {
      $manifest = Get-AppxPackageManifest -Package $package
      $application = @($manifest.Package.Applications.Application) | Where-Object Id -eq $parts[1] | Select-Object -First 1
      if ($application) {
        $executable = if ($application.Executable) { Join-Path $package.InstallLocation $application.Executable } else { '' }
        $visual = $application.'uap:VisualElements'
        if (-not $visual) { $visual = $application.VisualElements }
        if ($visual -and $visual.Square44x44Logo) { $icon = Join-Path $package.InstallLocation $visual.Square44x44Logo }
      }
    } catch { }
  }
  @(
    (B64 $start.Name), (B64 $start.AppID), (B64 $package.Name),
    (B64 $package.PackageFamilyName), (B64 $package.PackageFullName),
    (B64 $package.Publisher), (B64 $package.Version),
    (B64 $package.InstallLocation), (B64 $executable), (B64 $icon)
  ) -join "`t"
}
"#;

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
    fn parses_manifest_metadata_rows_portably() {
        let fields = [
            "Photos",
            "Microsoft.Windows.Photos_8wekyb3d8bbwe!App",
            "Microsoft.Windows.Photos",
            "Microsoft.Windows.Photos_8wekyb3d8bbwe",
            "Microsoft.Windows.Photos_2024.1.1.0_x64__8wekyb3d8bbwe",
            "CN=Microsoft Corporation",
            "2024.1.1.0",
            "C:\\Program Files\\WindowsApps\\Photos",
            "C:\\Program Files\\WindowsApps\\Photos\\Photos.exe",
            "C:\\Program Files\\WindowsApps\\Photos\\Assets\\StoreLogo.png",
        ];
        let encoded = fields
            .iter()
            .map(|field| encode_base64(field))
            .collect::<Vec<_>>()
            .join("\t");
        let app = parse_metadata_rows(&encoded).unwrap().remove(0);
        assert_eq!(app.package.package_family_name.as_deref(), Some(fields[3]));
        assert_eq!(app.package.application_id.as_deref(), Some("App"));
        assert_eq!(
            app.launch_target,
            "shell:AppsFolder\\Microsoft.Windows.Photos_8wekyb3d8bbwe!App"
        );
        assert_eq!(app.package.executable.as_deref(), Some(fields[8]));
        assert_eq!(app.icon_candidates[0], fields[9]);
    }

    fn encode_base64(value: &str) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = value.as_bytes();
        let mut output = String::new();
        for chunk in bytes.chunks(3) {
            let first = chunk[0] as u32;
            let second = chunk.get(1).copied().unwrap_or(0) as u32;
            let third = chunk.get(2).copied().unwrap_or(0) as u32;
            output.push(TABLE[((first >> 2) & 63) as usize] as char);
            output.push(TABLE[(((first & 3) << 4) | (second >> 4)) as usize] as char);
            output.push(if chunk.len() > 1 {
                TABLE[(((second & 15) << 2) | (third >> 6)) as usize] as char
            } else {
                '='
            });
            output.push(if chunk.len() > 2 {
                TABLE[(third & 63) as usize] as char
            } else {
                '='
            });
        }
        output
    }

    #[test]
    fn parses_manifest_application_attributes_portably() {
        let manifest = r#"<Applications><Application Id="Viewer" Executable="Viewer.exe"><uap:VisualElements Logo="Assets\Logo.png" /></Application></Applications>"#;
        assert_eq!(
            parse_manifest_application(manifest, Some("Viewer")),
            Some(ManifestApplication {
                executable: Some("Viewer.exe".to_owned()),
                logo: Some("Assets\\Logo.png".to_owned()),
            })
        );
        assert!(parse_manifest_application(manifest, Some("Other")).is_none());
    }

    #[test]
    fn rejects_malformed_metadata_rows() {
        assert!(matches!(
            parse_metadata_rows("not\tmetadata"),
            Err(WindowsAppError::Parse { .. })
        ));
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
            Err(error) => panic!("unexpected discovery error: {}", error),
        }
    }
}
