//! Platform boundary for command-line dispatch, target resolution, and launching.
//!
//! Planning is deliberately platform-neutral so it can be tested without starting
//! processes. Windows adapters can later implement the resolver and launcher traits
//! using ShellLink/Process APIs without leaking those details into the domain or UI.

use std::{env, ffi::OsString, fmt, path::PathBuf};

use crate::domain::{Category, ProgramShortcut};

const APP_USER_MODEL_ID_PREFIX: &str = "tjackenpacken.taskbarGroup.menu.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMode {
    Configure,
    Group { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    pub group_name: Option<String>,
}

impl LaunchRequest {
    pub fn from_environment() -> Self {
        Self::from_args(env::args_os())
    }

    /// Parse the legacy command line: the first argument after the executable is
    /// the group name, and its absence opens the configuration application.
    pub fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            group_name: args
                .into_iter()
                .skip(1)
                .next()
                .map(Into::into)
                .and_then(|value| value.into_string().ok()),
        }
    }

    pub fn mode(&self) -> LaunchMode {
        match &self.group_name {
            Some(name) => LaunchMode::Group { name: name.clone() },
            None => LaunchMode::Configure,
        }
    }

    pub fn app_user_model_id_for_group(&self) -> String {
        match &self.group_name {
            Some(name) => format!("{APP_USER_MODEL_ID_PREFIX}{name}"),
            None => "tjackenpacken.taskbarGroup.main".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutNumber {
    OneBased(u8),
}

impl ShortcutNumber {
    pub fn index(self) -> usize {
        match self {
            Self::OneBased(number) => number as usize - 1,
        }
    }

    pub fn from_key(key: char) -> Option<Self> {
        match key {
            '1'..='9' => Some(Self::OneBased(key as u8 - b'0')),
            '0' => Some(Self::OneBased(10)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchTrigger {
    Number(ShortcutNumber),
    CtrlEnter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKind {
    WindowsApp,
    Executable,
    Folder,
    ShellLink,
    UrlShortcut,
    Other,
}

impl TargetKind {
    pub fn classify(path: &str, is_windows_app: bool) -> Self {
        if is_windows_app {
            return Self::WindowsApp;
        }
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".lnk") {
            Self::ShellLink
        } else if lower.ends_with(".url") {
            Self::UrlShortcut
        } else if lower.ends_with(".exe") {
            Self::Executable
        } else if PathBuf::from(path).is_dir() {
            Self::Folder
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTarget {
    Path { path: String, kind: TargetKind },
    WindowsApp { app_user_model_id: String },
}

pub trait ShortcutResolver {
    fn resolve(&self, shortcut: &ProgramShortcut) -> Result<ResolvedTarget, ResolveError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    EmptyPath,
    UnsupportedWindowsShortcut { path: String },
    Failed { path: String, message: String },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => write!(f, "shortcut has no target path"),
            Self::UnsupportedWindowsShortcut { path } => {
                write!(f, "Windows shortcut resolution is unavailable for {path}")
            }
            Self::Failed { path, message } => write!(f, "could not resolve {path}: {message}"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// A dependency-free resolver useful for planning and for non-Windows callers.
/// `.lnk` files remain shell links; a Windows implementation may dereference them.
#[derive(Debug, Default, Clone, Copy)]
pub struct PassthroughResolver;

impl ShortcutResolver for PassthroughResolver {
    fn resolve(&self, shortcut: &ProgramShortcut) -> Result<ResolvedTarget, ResolveError> {
        if shortcut.file_path.trim().is_empty() {
            return Err(ResolveError::EmptyPath);
        }
        if shortcut.is_windows_app {
            return Ok(ResolvedTarget::WindowsApp {
                app_user_model_id: shortcut.file_path.clone(),
            });
        }
        Ok(ResolvedTarget::Path {
            path: shortcut.file_path.clone(),
            kind: TargetKind::classify(&shortcut.file_path, false),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub target: ResolvedTarget,
    pub arguments: String,
    pub working_directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    NoShortcut { index: usize },
    Resolve { index: usize, source: ResolveError },
}

pub struct LaunchPlanner<R> {
    resolver: R,
}

impl<R: ShortcutResolver> LaunchPlanner<R> {
    pub fn new(resolver: R) -> Self {
        Self { resolver }
    }

    pub fn plan(
        &self,
        category: &Category,
        trigger: LaunchTrigger,
    ) -> Result<Vec<LaunchSpec>, PlanError> {
        let indices: Vec<usize> = match trigger {
            LaunchTrigger::CtrlEnter if category.allow_open_all => {
                (0..category.shortcut_list.len()).collect()
            }
            LaunchTrigger::CtrlEnter => return Ok(Vec::new()),
            LaunchTrigger::Number(number) => vec![number.index()],
        };

        indices
            .into_iter()
            .map(|index| {
                let shortcut = category
                    .shortcut_list
                    .get(index)
                    .ok_or(PlanError::NoShortcut { index })?;
                let target = self
                    .resolver
                    .resolve(shortcut)
                    .map_err(|source| PlanError::Resolve { index, source })?;
                Ok(LaunchSpec {
                    target,
                    arguments: shortcut.arguments.clone(),
                    working_directory: shortcut.working_directory.clone(),
                })
            })
            .collect()
    }
}

pub trait Launcher {
    fn launch(&self, spec: &LaunchSpec) -> Result<(), LaunchError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    Unsupported,
    InvalidTarget {
        target: String,
        reason: String,
    },
    Process {
        target: String,
        message: String,
    },
    Shell {
        target: String,
        code: u32,
        message: String,
    },
    Failed(String),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "launching is unsupported on this platform"),
            Self::InvalidTarget { target, reason } => {
                write!(f, "invalid launch target {target}: {reason}")
            }
            Self::Process { target, message } => {
                write!(f, "could not start {target}: {message}")
            }
            Self::Shell {
                target,
                code,
                message,
            } => write!(
                f,
                "could not shell-launch {target} (code {code}): {message}"
            ),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LaunchError {}

pub mod shell_link;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::WindowsPlatform;

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(path: &str, app: bool) -> ProgramShortcut {
        ProgramShortcut {
            file_path: path.to_owned(),
            is_windows_app: app,
            name: String::new(),
            arguments: "--test".to_owned(),
            working_directory: "C:\\work".to_owned(),
        }
    }

    #[test]
    fn parses_legacy_positional_group_mode() {
        assert_eq!(
            LaunchRequest::from_args(["app.exe"]),
            LaunchRequest { group_name: None }
        );
        let request = LaunchRequest::from_args(["app.exe", "Games"]);
        assert_eq!(
            request.mode(),
            LaunchMode::Group {
                name: "Games".to_owned()
            }
        );
        assert_eq!(
            request.app_user_model_id_for_group(),
            "tjackenpacken.taskbarGroup.menu.Games"
        );
    }

    #[test]
    fn maps_zero_to_the_tenth_shortcut() {
        assert_eq!(ShortcutNumber::from_key('0').unwrap().index(), 9);
        assert_eq!(ShortcutNumber::from_key('9').unwrap().index(), 8);
        assert!(ShortcutNumber::from_key('a').is_none());
    }

    #[test]
    fn plans_numbered_shortcut_without_launching_it() {
        let mut category = Category::new("Test");
        category.shortcut_list = vec![shortcut("one.exe", false), shortcut("two.lnk", false)];
        let plan = LaunchPlanner::new(PassthroughResolver)
            .plan(
                &category,
                LaunchTrigger::Number(ShortcutNumber::OneBased(2)),
            )
            .unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].arguments, "--test");
        assert_eq!(
            plan[0].target,
            ResolvedTarget::Path {
                path: "two.lnk".to_owned(),
                kind: TargetKind::ShellLink
            }
        );
    }

    #[test]
    fn ctrl_enter_is_gated_by_allow_open_all() {
        let mut category = Category::new("Test");
        category.shortcut_list = vec![shortcut("one.exe", false), shortcut("store-id", true)];
        let planner = LaunchPlanner::new(PassthroughResolver);
        assert!(planner
            .plan(&category, LaunchTrigger::CtrlEnter)
            .unwrap()
            .is_empty());
        category.allow_open_all = true;
        let plan = planner.plan(&category, LaunchTrigger::CtrlEnter).unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(
            plan[1].target,
            ResolvedTarget::WindowsApp {
                app_user_model_id: "store-id".to_owned()
            }
        );
    }

    #[test]
    fn missing_numbered_shortcut_is_reported() {
        let category = Category::new("Test");
        let error = LaunchPlanner::new(PassthroughResolver)
            .plan(
                &category,
                LaunchTrigger::Number(ShortcutNumber::OneBased(1)),
            )
            .unwrap_err();
        assert_eq!(error, PlanError::NoShortcut { index: 0 });
    }
}
