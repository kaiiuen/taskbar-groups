//! GUI-independent orchestration for the legacy configuration and launcher flows.
//!
//! The controller owns user intent and persistence coordination. A future native
//! Windows frontend should translate controls and window events into `Action`s,
//! then render `View`; it should not contain business-flow decisions.

mod shell;

use std::{fs, io};

use crate::{
    domain::{Category, ProgramShortcut, ValidationError, MAX_SHORTCUTS},
    persistence::AppPaths,
    platform::{LaunchPlanner, LaunchRequest, LaunchTrigger, PassthroughResolver, ShortcutNumber},
};

pub use shell::{TemporaryShell, UiShell};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Configuration,
    GroupLauncher,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    ReloadGroups,
    BeginNewGroup,
    EditGroup(String),
    AddShortcut { path: String, is_windows_app: bool },
    RemoveShortcut(usize),
    MoveShortcut { from: usize, to: usize },
    SelectShortcut(Option<usize>),
    SetShortcutName(String),
    SetArguments(String),
    SetWorkingDirectory(String),
    SetGroupName(String),
    SetColor(String),
    SetWidth(i32),
    SetOpacity(f64),
    SetAllowOpenAll(bool),
    SetIcon { path: String },
    SaveGroup,
    DeleteGroup,
    CancelEditor,
    RevealGroup(String),
    Key(char),
    CtrlEnter,
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub screen: Screen,
    pub groups: Vec<String>,
    pub editor: Option<Category>,
    pub selected_shortcut: Option<usize>,
    pub icon_path: Option<String>,
    pub error: Option<UiError>,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UiError {
    Load { group: String, message: String },
    Save(String),
    Delete(String),
    Validation(ValidationError),
    ShortcutLimit { maximum: usize },
    NoEditor,
    NoSelection,
    InvalidSelection { index: usize },
    Launch { index: usize, message: String },
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load { group, message } => write!(f, "Could not load group {group}: {message}"),
            Self::Save(message) => write!(f, "Could not save group: {message}"),
            Self::Delete(message) => write!(f, "Could not delete group: {message}"),
            Self::Validation(error) => write!(f, "Group is not valid: {error}"),
            Self::ShortcutLimit { maximum } => {
                write!(f, "A group can contain at most {maximum} shortcuts")
            }
            Self::NoEditor => write!(f, "No group editor is open"),
            Self::NoSelection => write!(f, "Select a shortcut first"),
            Self::InvalidSelection { index } => write!(f, "Shortcut {index} does not exist"),
            Self::Launch { index, message } => {
                write!(f, "Could not launch shortcut {index}: {message}")
            }
        }
    }
}

pub struct Controller<S> {
    paths: AppPaths,
    shell: S,
    view: View,
    editing_existing: bool,
}

impl<S: UiShell> Controller<S> {
    pub fn new(request: LaunchRequest, paths: AppPaths, shell: S) -> Self {
        let screen = match request.mode() {
            crate::platform::LaunchMode::Configure => Screen::Configuration,
            crate::platform::LaunchMode::Group { .. } => Screen::GroupLauncher,
        };
        let group_name = match &request.group_name {
            Some(name) => Some(name.clone()),
            None => None,
        };
        let mut controller = Self {
            paths,
            shell,
            view: View {
                screen,
                groups: Vec::new(),
                editor: None,
                selected_shortcut: None,
                icon_path: None,
                error: None,
                notice: None,
            },
            editing_existing: false,
        };
        if let Some(name) = group_name {
            controller.edit_group(&name);
        }
        controller
    }

    pub fn view(&self) -> &View {
        &self.view
    }
    pub fn shell_mut(&mut self) -> &mut S {
        &mut self.shell
    }

    pub fn dispatch(&mut self, action: Action) {
        self.view.error = None;
        self.view.notice = None;
        match action {
            Action::ReloadGroups => self.reload_groups(),
            Action::BeginNewGroup => {
                self.editing_existing = false;
                self.view.editor = Some(Category::default());
                self.view.selected_shortcut = None;
                self.view.icon_path = None;
            }
            Action::EditGroup(name) => self.edit_group(&name),
            Action::AddShortcut {
                path,
                is_windows_app,
            } => self.add_shortcut(path, is_windows_app),
            Action::RemoveShortcut(index) => self.remove_shortcut(index),
            Action::MoveShortcut { from, to } => self.move_shortcut(from, to),
            Action::SelectShortcut(index) => self.select(index),
            Action::SetShortcutName(value) => self.update_selected(|s| s.name = value),
            Action::SetArguments(value) => self.update_selected(|s| s.arguments = value),
            Action::SetWorkingDirectory(value) => {
                self.update_selected(|s| s.working_directory = value)
            }
            Action::SetGroupName(value) => self.update_editor(|g| g.name = value),
            Action::SetColor(value) => self.update_editor(|g| g.color_string = value),
            Action::SetWidth(value) => self.update_editor(|g| g.width = value),
            Action::SetOpacity(value) => self.update_editor(|g| g.opacity = value),
            Action::SetAllowOpenAll(value) => self.update_editor(|g| g.allow_open_all = value),
            Action::SetIcon { path } => self.view.icon_path = Some(path),
            Action::SaveGroup => self.save_group(),
            Action::DeleteGroup => self.delete_group(),
            Action::CancelEditor => {
                self.view.editor = None;
                self.view.selected_shortcut = None;
                self.view.icon_path = None;
            }
            Action::RevealGroup(name) => self.reveal_group(&name),
            Action::Key(key) => {
                if let Some(number) = ShortcutNumber::from_key(key) {
                    self.launch(LaunchTrigger::Number(number));
                }
            }
            Action::CtrlEnter => self.launch(LaunchTrigger::CtrlEnter),
        }
    }

    fn reload_groups(&mut self) {
        self.view.groups.clear();
        let entries = match fs::read_dir(&self.paths.config) {
            Ok(entries) => entries,
            Err(error) => {
                self.view.error = Some(UiError::Load {
                    group: "configuration".into(),
                    message: error.to_string(),
                });
                return;
            }
        };
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            match self.paths.load_group(&name) {
                Ok(_) => self.view.groups.push(name),
                Err(error) => {
                    self.view.error = Some(UiError::Load {
                        group: name,
                        message: error.to_string(),
                    })
                }
            }
        }
        self.view.groups.sort();
    }

    fn edit_group(&mut self, name: &str) {
        match self.paths.load_group(name) {
            Ok(category) => {
                self.editing_existing = true;
                self.view.editor = Some(category);
                self.view.selected_shortcut = None;
                self.view.icon_path = None;
            }
            Err(error) => {
                self.view.error = Some(UiError::Load {
                    group: name.into(),
                    message: error.to_string(),
                })
            }
        }
    }
    fn add_shortcut(&mut self, path: String, is_windows_app: bool) {
        self.update_editor(|group| {
            if group.shortcut_list.len() < MAX_SHORTCUTS {
                let mut shortcut = ProgramShortcut::new(path);
                shortcut.is_windows_app = is_windows_app;
                group.shortcut_list.push(shortcut);
            }
        });
        if self
            .view
            .editor
            .as_ref()
            .is_some_and(|g| g.shortcut_list.len() >= MAX_SHORTCUTS)
        {
            self.view.notice = Some(format!(
                "A group can contain at most {MAX_SHORTCUTS} shortcuts"
            ));
        }
    }
    fn remove_shortcut(&mut self, index: usize) {
        if let Some(group) = self.view.editor.as_mut() {
            if index < group.shortcut_list.len() {
                group.shortcut_list.remove(index);
                self.view.selected_shortcut = None;
            } else {
                self.fail(UiError::InvalidSelection { index });
            }
        } else {
            self.fail(UiError::NoEditor);
        }
    }
    fn move_shortcut(&mut self, from: usize, to: usize) {
        if let Some(group) = self.view.editor.as_mut() {
            if from < group.shortcut_list.len() && to < group.shortcut_list.len() {
                group.shortcut_list.swap(from, to);
                self.view.selected_shortcut = Some(to);
            } else {
                self.fail(UiError::InvalidSelection {
                    index: from.max(to),
                });
            }
        } else {
            self.fail(UiError::NoEditor);
        }
    }
    fn select(&mut self, index: Option<usize>) {
        if let Some(i) = index {
            if self
                .view
                .editor
                .as_ref()
                .is_none_or(|g| i >= g.shortcut_list.len())
            {
                self.fail(UiError::InvalidSelection { index: i });
                return;
            }
        }
        self.view.selected_shortcut = index;
    }
    fn update_selected<F: FnOnce(&mut ProgramShortcut)>(&mut self, update: F) {
        if let (Some(group), Some(index)) = (self.view.editor.as_mut(), self.view.selected_shortcut)
        {
            if let Some(shortcut) = group.shortcut_list.get_mut(index) {
                update(shortcut);
                return;
            }
        }
        self.fail(UiError::NoSelection);
    }
    fn update_editor<F: FnOnce(&mut Category)>(&mut self, update: F) {
        if let Some(group) = self.view.editor.as_mut() {
            update(group);
        } else {
            self.fail(UiError::NoEditor);
        }
    }
    fn save_group(&mut self) {
        let Some(group) = self.view.editor.clone() else {
            self.fail(UiError::NoEditor);
            return;
        };
        if let Err(error) = group.validate() {
            self.fail(UiError::Validation(error));
            return;
        }
        if self.view.icon_path.is_none() && self.editing_existing { /* Existing icon is retained by the native adapter. */
        } else if self.view.icon_path.is_none() {
            self.fail(UiError::Save("select a group icon".into()));
            return;
        }
        if let Err(error) = self.paths.save_group(&group) {
            self.fail(UiError::Save(error.to_string()));
            return;
        }
        self.view.notice = Some(format!("Saved group {}", group.name));
        self.view.editor = None;
        self.reload_groups();
    }
    fn delete_group(&mut self) {
        let Some(group) = self.view.editor.as_ref() else {
            self.fail(UiError::NoEditor);
            return;
        };
        if let Err(error) = self.paths.delete_group(&group.name) {
            self.fail(UiError::Delete(error.to_string()));
            return;
        }
        self.view.editor = None;
        self.reload_groups();
    }
    fn reveal_group(&mut self, name: &str) {
        match self.paths.group(name) {
            Ok(group) => self
                .shell
                .reveal_group(&group.shortcut_path(&self.paths).display().to_string()),
            Err(error) => self.fail(UiError::Delete(error.to_string())),
        }
    }
    fn launch(&mut self, trigger: LaunchTrigger) {
        if self.view.editor.is_none() {
            self.fail(UiError::NoEditor);
            return;
        }
        let trigger = match trigger {
            LaunchTrigger::Number(number) => LaunchTrigger::Number(number),
            other => other,
        };
        let Some(group) = self.view.editor.clone() else {
            return;
        };
        self.launch_plans(&group, trigger);
    }
    fn launch_plans(&mut self, group: &Category, trigger: LaunchTrigger) {
        let planner = LaunchPlanner::new(PassthroughResolver);
        match planner.plan(group, trigger) {
            Ok(specs) => {
                for (index, spec) in specs.iter().enumerate() {
                    if let Err(error) = self.shell.launch(spec) {
                        self.fail(UiError::Launch {
                            index,
                            message: error.to_string(),
                        });
                    }
                }
            }
            Err(crate::platform::PlanError::NoShortcut { index }) => {
                self.fail(UiError::InvalidSelection { index })
            }
            Err(crate::platform::PlanError::Resolve { index, source }) => {
                self.fail(UiError::Launch {
                    index,
                    message: source.to_string(),
                })
            }
        }
    }
    fn fail(&mut self, error: UiError) {
        self.shell.show_error(&error.to_string());
        self.view.error = Some(error);
    }
}

pub fn run(request: LaunchRequest, paths: AppPaths) -> io::Result<()> {
    let mut controller = Controller::new(request, paths, TemporaryShell);
    if controller.view.screen == Screen::Configuration {
        controller.dispatch(Action::ReloadGroups);
        controller
            .shell_mut()
            .show_message("Taskbar Groups configuration mode");
    } else {
        controller
            .shell_mut()
            .show_message("Taskbar Groups group launcher mode");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct TestShell {
        launched: Rc<RefCell<usize>>,
        errors: Rc<RefCell<Vec<String>>>,
    }
    impl UiShell for TestShell {
        fn show_message(&mut self, _: &str) {}
        fn show_error(&mut self, message: &str) {
            self.errors.borrow_mut().push(message.into());
        }
        fn launch(&mut self, _: &crate::platform::LaunchSpec) -> Result<(), io::Error> {
            *self.launched.borrow_mut() += 1;
            Ok(())
        }
        fn reveal_group(&mut self, _: &str) {}
    }

    fn controller() -> Controller<TestShell> {
        Controller::new(
            LaunchRequest { group_name: None },
            AppPaths::from_root(std::env::temp_dir().join("taskbar-groups-ui-test")),
            TestShell::default(),
        )
    }
    #[test]
    fn creates_and_edits_shortcuts_without_gui() {
        let mut c = controller();
        c.dispatch(Action::BeginNewGroup);
        c.dispatch(Action::SetGroupName("Games".into()));
        c.dispatch(Action::AddShortcut {
            path: "play.exe".into(),
            is_windows_app: false,
        });
        c.dispatch(Action::SelectShortcut(Some(0)));
        c.dispatch(Action::SetArguments("--safe".into()));
        assert_eq!(
            c.view().editor.as_ref().unwrap().shortcut_list[0].arguments,
            "--safe"
        );
    }
    #[test]
    fn rejects_more_than_twenty_shortcuts() {
        let mut c = controller();
        c.dispatch(Action::BeginNewGroup);
        for i in 0..MAX_SHORTCUTS {
            c.dispatch(Action::AddShortcut {
                path: format!("{i}.exe"),
                is_windows_app: false,
            });
        }
        c.dispatch(Action::AddShortcut {
            path: "extra.exe".into(),
            is_windows_app: false,
        });
        assert_eq!(
            c.view().editor.as_ref().unwrap().shortcut_list.len(),
            MAX_SHORTCUTS
        );
    }
    #[test]
    fn launcher_uses_number_and_open_all_rules() {
        let shell = TestShell::default();
        let launched = shell.launched.clone();
        let mut c = Controller::new(
            LaunchRequest {
                group_name: Some("Games".into()),
            },
            AppPaths::from_root("test"),
            shell,
        );
        c.dispatch(Action::BeginNewGroup);
        c.dispatch(Action::AddShortcut {
            path: "one.exe".into(),
            is_windows_app: false,
        });
        c.dispatch(Action::AddShortcut {
            path: "two.exe".into(),
            is_windows_app: false,
        });
        c.dispatch(Action::Key('2'));
        assert_eq!(*launched.borrow(), 1);
        c.dispatch(Action::SetAllowOpenAll(true));
        c.dispatch(Action::CtrlEnter);
        assert_eq!(*launched.borrow(), 3);
    }
    #[test]
    fn validation_is_presented_as_ui_error() {
        let mut c = controller();
        c.dispatch(Action::BeginNewGroup);
        c.dispatch(Action::SaveGroup);
        assert!(matches!(c.view().error, Some(UiError::Validation(_))));
    }
}
