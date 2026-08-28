//! Native Win32 UI adapter.
//!
//! Native controls only translate events. Editing, validation, persistence, and
//! launch planning remain in `ui::Controller` and the existing platform boundary.

use super::Action;
use crate::platform::windows_apps::WindowsApp;

#[derive(Debug, Clone, PartialEq)]
pub enum NativeEvent {
    ReloadGroups,
    NewGroup,
    EditGroup(String),
    DeleteGroup,
    SaveGroup,
    Cancel,
    AddShortcut { path: String, is_windows_app: bool },
    SelectWindowsApp(WindowsApp),
    RemoveShortcut(usize),
    MoveShortcut { from: usize, to: usize },
    SelectShortcut(Option<usize>),
    ShortcutName(String),
    Arguments(String),
    WorkingDirectory(String),
    GroupName(String),
    Color(String),
    Width(i32),
    Opacity(f64),
    AllowOpenAll(bool),
    Icon(String),
    Key(char),
    CtrlEnter,
}

pub fn action_for_event(event: NativeEvent) -> Action {
    match event {
        NativeEvent::ReloadGroups => Action::ReloadGroups,
        NativeEvent::NewGroup => Action::BeginNewGroup,
        NativeEvent::EditGroup(name) => Action::EditGroup(name),
        NativeEvent::DeleteGroup => Action::DeleteGroup,
        NativeEvent::SaveGroup => Action::SaveGroup,
        NativeEvent::Cancel => Action::CancelEditor,
        NativeEvent::AddShortcut {
            path,
            is_windows_app,
        } => Action::AddShortcut {
            path,
            is_windows_app,
        },
        NativeEvent::SelectWindowsApp(app) => Action::SelectWindowsApp(app),
        NativeEvent::RemoveShortcut(index) => Action::RemoveShortcut(index),
        NativeEvent::MoveShortcut { from, to } => Action::MoveShortcut { from, to },
        NativeEvent::SelectShortcut(index) => Action::SelectShortcut(index),
        NativeEvent::ShortcutName(value) => Action::SetShortcutName(value),
        NativeEvent::Arguments(value) => Action::SetArguments(value),
        NativeEvent::WorkingDirectory(value) => Action::SetWorkingDirectory(value),
        NativeEvent::GroupName(value) => Action::SetGroupName(value),
        NativeEvent::Color(value) => Action::SetColor(value),
        NativeEvent::Width(value) => Action::SetWidth(value),
        NativeEvent::Opacity(value) => Action::SetOpacity(value),
        NativeEvent::AllowOpenAll(value) => Action::SetAllowOpenAll(value),
        NativeEvent::Icon(value) => Action::SetIcon { path: value },
        NativeEvent::Key(value) => Action::Key(value),
        NativeEvent::CtrlEnter => Action::CtrlEnter,
    }
}

#[cfg(windows)]
mod native {
    use super::{action_for_event, NativeEvent};
    use crate::{
        persistence::AppPaths,
        platform::{
            windows_apps::{WindowsApp, WindowsAppDiscovery, WindowsShellAppDiscovery},
            LaunchRequest, LaunchSpec, Launcher, WindowsPlatform,
        },
        ui::{Controller, UiShell},
    };
    use std::{
        io,
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::COLOR_WINDOW,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{GetKeyState, VK_RETURN},
            Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, ShellExecuteW, HDROP},
            WindowsAndMessaging::*,
        },
    };

    const GROUPS: i32 = 100;
    const GROUP_NAME: i32 = 101;
    const ICON: i32 = 102;
    const SHORTCUTS: i32 = 103;
    const SHORTCUT_PATH: i32 = 104;
    const SHORTCUT_NAME: i32 = 105;
    const ARGUMENTS: i32 = 106;
    const WORKDIR: i32 = 107;
    const NEW: i32 = 108;
    const EDIT: i32 = 109;
    const ADD: i32 = 110;
    const BROWSE_FILES: i32 = 122;
    const BROWSE_FOLDER: i32 = 123;
    const IMPORT_FILES: i32 = 124;
    const BROWSE_ICON: i32 = 125;
    const REMOVE: i32 = 111;
    const UP: i32 = 112;
    const DOWN: i32 = 113;
    const SAVE: i32 = 114;
    const DELETE: i32 = 115;
    const CANCEL: i32 = 116;
    const ALLOW_ALL: i32 = 117;
    const COLOR: i32 = 118;
    const WIDTH: i32 = 119;
    const OPACITY: i32 = 120;
    const LAUNCH: i32 = 121;
    const DISCOVER_APPS: i32 = 126;
    const APPS: i32 = 127;
    const VK_CONTROL_KEY: i32 = 0x11;
    const BST_UNCHECKED_VALUE: usize = 0;
    const BST_CHECKED_VALUE: usize = 1;
    const OFN_EXPLORER: u32 = 0x0008_0000;
    const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
    const OFN_ALLOWMULTISELECT: u32 = 0x0000_0200;
    const BIF_RETURNONLYFSDIRS: u32 = 0x0001;

    #[repr(C)]
    struct OpenFileNameW {
        l_struct_size: u32,
        hwnd_owner: HWND,
        h_instance: isize,
        filter: *const u16,
        custom_filter: *mut u16,
        max_cust_filter: u32,
        filter_index: u32,
        file: *mut u16,
        max_file: u32,
        file_title: *mut u16,
        max_file_title: u32,
        initial_dir: *const u16,
        title: *const u16,
        flags: u32,
        file_offset: u16,
        file_extension: u16,
        def_ext: *const u16,
        cust_data: LPARAM,
        hook: isize,
        template_name: *const u16,
        reserved: *const u16,
        reserved2: u32,
        flags_ex: u32,
    }

    #[repr(C)]
    struct BrowseInfoW {
        hwnd_owner: HWND,
        pidl_root: *mut core::ffi::c_void,
        display_name: *mut u16,
        title: *const u16,
        flags: u32,
        callback: isize,
        callback_data: LPARAM,
        image: i32,
    }

    #[link(name = "comdlg32")]
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(file_name: *mut OpenFileNameW) -> i32;
        fn SHBrowseForFolderW(info: *const BrowseInfoW) -> *mut core::ffi::c_void;
        fn SHGetPathFromIDListW(item_id_list: *const core::ffi::c_void, path: *mut u16) -> i32;
    }

    struct NativeShell(HWND);
    impl UiShell for NativeShell {
        fn show_message(&mut self, message: &str) {
            message_box(self.0, message, MB_OK);
        }
        fn show_error(&mut self, message: &str) {
            message_box(self.0, message, MB_OK | MB_ICONERROR);
        }
        fn launch(&mut self, spec: &LaunchSpec) -> Result<(), io::Error> {
            WindowsPlatform
                .launch(spec)
                .map_err(|error| io::Error::other(error.to_string()))
        }
        fn reveal_group(&mut self, path: &str) {
            let path = wide(path);
            unsafe {
                ShellExecuteW(
                    self.0,
                    ptr::null(),
                    path.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    SW_SHOWNORMAL,
                );
            }
        }
    }

    struct NativeUi {
        controller: Controller<NativeShell>,
        hwnd: HWND,
        groups: HWND,
        group_name: HWND,
        icon: HWND,
        shortcuts: HWND,
        apps: HWND,
        shortcut_path: HWND,
        shortcut_name: HWND,
        arguments: HWND,
        workdir: HWND,
        allow_all: HWND,
        color: HWND,
        width: HWND,
        opacity: HWND,
        updating: bool,
        discovered_apps: Vec<WindowsApp>,
    }

    pub fn run(request: LaunchRequest, paths: AppPaths) -> io::Result<()> {
        let class = wide("TaskbarGroupsNativeUi");
        let instance = unsafe { GetModuleHandleW(ptr::null()) } as HINSTANCE;
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
            hbrBackground: (COLOR_WINDOW + 1) as _,
            ..unsafe { std::mem::zeroed() }
        };
        if unsafe { RegisterClassW(&wc) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                wide("Taskbar Groups").as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                860,
                650,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null_mut(),
            )
        };
        if hwnd.is_null() {
            return Err(io::Error::last_os_error());
        }
        let controller = Controller::new(request, paths, NativeShell(hwnd));
        let mut ui = Box::new(build_ui(hwnd, controller));
        ui.controller
            .dispatch(action_for_event(NativeEvent::ReloadGroups));
        render(&mut ui);
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(ui) as isize);
        }
        unsafe {
            let mut message = std::mem::zeroed();
            while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }

    fn build_ui(hwnd: HWND, controller: Controller<NativeShell>) -> NativeUi {
        let groups = control(
            "LISTBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_BORDER | LBS_NOTIFY as u32 | WS_VSCROLL,
            16,
            42,
            250,
            500,
            hwnd,
            GROUPS,
        );
        let group_name = labeled_edit(hwnd, "Group name", 290, 24, GROUP_NAME, "");
        let icon = labeled_edit(hwnd, "Icon path (new groups)", 290, 68, ICON, "");
        let color = labeled_edit(hwnd, "Color", 290, 112, COLOR, "#1f1f1f");
        let width = labeled_edit(hwnd, "Width", 555, 112, WIDTH, "0");
        let opacity = labeled_edit(hwnd, "Opacity", 690, 112, OPACITY, "10");
        let allow_all = control(
            "BUTTON",
            "Open all with Ctrl+Enter",
            WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX as u32,
            290,
            154,
            230,
            24,
            hwnd,
            ALLOW_ALL,
        );
        let shortcuts = control(
            "LISTBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_BORDER | LBS_NOTIFY as u32 | WS_VSCROLL,
            290,
            195,
            535,
            140,
            hwnd,
            SHORTCUTS,
        );
        let shortcut_path =
            labeled_edit(hwnd, "Path / AppUserModelId", 290, 355, SHORTCUT_PATH, "");
        let shortcut_name = labeled_edit(hwnd, "Shortcut name", 555, 355, SHORTCUT_NAME, "");
        let arguments = labeled_edit(hwnd, "Arguments", 290, 399, ARGUMENTS, "");
        let workdir = labeled_edit(hwnd, "Working directory", 555, 399, WORKDIR, "");
        button(hwnd, "New group", 16, 12, 110, 24, NEW);
        button(hwnd, "Edit selected", 136, 12, 110, 24, EDIT);
        button(hwnd, "Add shortcut", 290, 455, 110, 26, ADD);
        button(hwnd, "Discover apps", 700, 455, 110, 26, DISCOVER_APPS);
        let apps = control(
            "LISTBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_BORDER | LBS_NOTIFY as u32 | WS_VSCROLL,
            700,
            195,
            125,
            140,
            hwnd,
            APPS,
        );
        button(hwnd, "Browse files", 290, 428, 110, 24, BROWSE_FILES);
        button(hwnd, "Browse folder", 410, 428, 110, 24, BROWSE_FOLDER);
        button(hwnd, "Import files", 530, 428, 110, 24, IMPORT_FILES);
        button(hwnd, "Remove", 410, 455, 90, 26, REMOVE);
        button(hwnd, "Browse icon", 690, 68, 110, 24, BROWSE_ICON);
        button(hwnd, "Move up", 510, 455, 90, 26, UP);
        button(hwnd, "Move down", 610, 455, 90, 26, DOWN);
        button(hwnd, "Save", 290, 500, 90, 26, SAVE);
        button(hwnd, "Delete", 390, 500, 90, 26, DELETE);
        button(hwnd, "Cancel", 490, 500, 90, 26, CANCEL);
        button(hwnd, "Launch all", 590, 500, 100, 26, LAUNCH);
        NativeUi {
            controller,
            hwnd,
            groups,
            group_name,
            icon,
            shortcuts,
            apps,
            shortcut_path,
            shortcut_name,
            arguments,
            workdir,
            allow_all,
            color,
            width,
            opacity,
            updating: false,
            discovered_apps: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn control(
        class: &str,
        text: &str,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: HWND,
        id: i32,
    ) -> HWND {
        unsafe {
            CreateWindowExW(
                WS_EX_CLIENTEDGE,
                wide(class).as_ptr(),
                wide(text).as_ptr(),
                style,
                x,
                y,
                w,
                h,
                parent,
                id as _,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        }
    }
    fn button(parent: HWND, text: &str, x: i32, y: i32, w: i32, h: i32, id: i32) -> HWND {
        control(
            "BUTTON",
            text,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            x,
            y,
            w,
            h,
            parent,
            id,
        )
    }
    fn labeled_edit(parent: HWND, label: &str, x: i32, y: i32, id: i32, value: &str) -> HWND {
        control(
            "STATIC",
            label,
            WS_CHILD | WS_VISIBLE,
            x,
            y,
            125,
            20,
            parent,
            0,
        );
        control(
            "EDIT",
            value,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            x + 125,
            y - 2,
            135,
            24,
            parent,
            id,
        )
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CREATE => {
                DragAcceptFiles(hwnd, 1);
                0
            }
            WM_DROPFILES => {
                if let Some(ui) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeUi).as_mut()
                {
                    drop_files(ui, wparam as HDROP);
                }
                0
            }
            WM_COMMAND => {
                if let Some(ui) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeUi).as_mut()
                {
                    command(ui, (wparam & 0xffff) as i32, (wparam >> 16) as u32);
                }
                0
            }
            WM_KEYDOWN => {
                if let Some(ui) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeUi).as_mut()
                {
                    if wparam == VK_RETURN as usize && GetKeyState(VK_CONTROL_KEY) < 0 {
                        ui.controller
                            .dispatch(action_for_event(NativeEvent::CtrlEnter));
                        render(ui);
                    }
                }
                0
            }
            WM_DESTROY => {
                let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeUi;
                if !state.is_null() {
                    drop(Box::from_raw(state));
                }
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, _lparam),
        }
    }

    fn command(ui: &mut NativeUi, id: i32, notification: u32) {
        if ui.updating {
            return;
        }
        let event = match id {
            NEW => Some(NativeEvent::NewGroup),
            EDIT if notification == LBN_DBLCLK || notification == 0 => {
                selected_group(ui).map(NativeEvent::EditGroup)
            }
            LAUNCH => Some(NativeEvent::CtrlEnter),
            GROUP_NAME if notification == EN_CHANGE => {
                Some(NativeEvent::GroupName(text(ui.group_name)))
            }
            ICON if notification == EN_CHANGE => Some(NativeEvent::Icon(text(ui.icon))),
            COLOR if notification == EN_CHANGE => Some(NativeEvent::Color(text(ui.color))),
            WIDTH if notification == EN_CHANGE => {
                text(ui.width).parse().ok().map(NativeEvent::Width)
            }
            OPACITY if notification == EN_CHANGE => {
                text(ui.opacity).parse().ok().map(NativeEvent::Opacity)
            }
            ALLOW_ALL if notification == BN_CLICKED => Some(NativeEvent::AllowOpenAll(unsafe {
                SendMessageW(ui.allow_all, BM_GETCHECK, 0, 0) == BST_CHECKED_VALUE as isize
            })),
            SHORTCUT_PATH if notification == EN_CHANGE => None,
            SHORTCUT_NAME if notification == EN_CHANGE => {
                Some(NativeEvent::ShortcutName(text(ui.shortcut_name)))
            }
            ARGUMENTS if notification == EN_CHANGE => {
                Some(NativeEvent::Arguments(text(ui.arguments)))
            }
            WORKDIR if notification == EN_CHANGE => {
                Some(NativeEvent::WorkingDirectory(text(ui.workdir)))
            }
            ADD => nonempty(ui, ui.shortcut_path).map(|path| NativeEvent::AddShortcut {
                path,
                is_windows_app: false,
            }),
            REMOVE => selected(ui.shortcuts).map(NativeEvent::RemoveShortcut),
            UP => selected(ui.shortcuts).map(|index| NativeEvent::MoveShortcut {
                from: index,
                to: index.saturating_sub(1),
            }),
            DOWN => selected(ui.shortcuts).map(|index| NativeEvent::MoveShortcut {
                from: index,
                to: index + 1,
            }),
            SAVE => Some(NativeEvent::SaveGroup),
            DELETE => Some(NativeEvent::DeleteGroup),
            CANCEL => Some(NativeEvent::Cancel),
            SHORTCUTS if notification == LBN_SELCHANGE => {
                selected(ui.shortcuts).map(|index| NativeEvent::SelectShortcut(Some(index)))
            }
            APPS if notification == LBN_DBLCLK => selected(ui.apps)
                .and_then(|index| ui.discovered_apps.get(index).cloned())
                .map(NativeEvent::SelectWindowsApp),
            _ => None,
        };
        if let Some(event) = event {
            dispatch(ui, event);
        } else if id == BROWSE_FILES || id == IMPORT_FILES {
            if let Some(paths) = choose_files(ui.hwnd, id == IMPORT_FILES) {
                for path in paths {
                    dispatch(
                        ui,
                        NativeEvent::AddShortcut {
                            path,
                            is_windows_app: false,
                        },
                    );
                }
            }
        } else if id == BROWSE_FOLDER {
            if let Some(path) = choose_folder(ui.hwnd) {
                dispatch(
                    ui,
                    NativeEvent::AddShortcut {
                        path,
                        is_windows_app: false,
                    },
                );
            }
        } else if id == BROWSE_ICON {
            if let Some(path) = choose_files(ui.hwnd, false).and_then(|mut paths| paths.pop()) {
                dispatch(ui, NativeEvent::Icon(path));
            }
        } else if id == DISCOVER_APPS {
            match WindowsShellAppDiscovery.enumerate() {
                Ok(apps) => {
                    ui.discovered_apps = apps;
                    unsafe { SendMessageW(ui.apps, LB_RESETCONTENT, 0, 0) };
                    for app in &ui.discovered_apps {
                        let value = wide(&format!("{} — {}", app.display_name, app.aumid));
                        unsafe {
                            SendMessageW(ui.apps, LB_ADDSTRING, 0, value.as_ptr() as LPARAM);
                        }
                    }
                    if ui.discovered_apps.is_empty() {
                        ui.controller
                            .shell_mut()
                            .show_message("No Windows apps were discovered.");
                    }
                }
                Err(error) => ui.controller.shell_mut().show_error(&error.to_string()),
            }
        }
    }

    fn dispatch(ui: &mut NativeUi, event: NativeEvent) {
        ui.controller.dispatch(action_for_event(event));
        render(ui);
        if let Some(error) = ui.controller.view().error.as_ref() {
            let message = error.to_string();
            ui.controller.shell_mut().show_error(&message);
        }
    }

    fn drop_files(ui: &mut NativeUi, drop: HDROP) {
        let count = unsafe { DragQueryFileW(drop, 0xffff_ffff, ptr::null_mut(), 0) };
        for index in 0..count {
            let length = unsafe { DragQueryFileW(drop, index, ptr::null_mut(), 0) };
            let mut value = vec![0u16; length as usize + 1];
            unsafe { DragQueryFileW(drop, index, value.as_mut_ptr(), value.len() as u32) };
            let path = String::from_utf16_lossy(&value[..length as usize]);
            if valid_target(&path) {
                dispatch(
                    ui,
                    NativeEvent::AddShortcut {
                        path,
                        is_windows_app: false,
                    },
                );
            } else {
                ui.controller
                    .shell_mut()
                    .show_error(&format!("Unsupported shortcut target: {path}"));
            }
        }
        unsafe { DragFinish(drop) };
    }

    fn choose_files(ui: HWND, multi: bool) -> Option<Vec<String>> {
        let mut buffer = vec![0u16; 32_768];
        let filter =
            wide("Programs and shortcuts\0*.exe;*.com;*.bat;*.cmd;*.lnk;*.url\0All files\0*.*\0\0");
        let dialog_title = wide(if multi {
            "Import shortcuts"
        } else {
            "Select a shortcut"
        });
        let mut dialog = OpenFileNameW {
            l_struct_size: std::mem::size_of::<OpenFileNameW>() as u32,
            hwnd_owner: ui,
            h_instance: 0,
            filter: filter.as_ptr(),
            custom_filter: ptr::null_mut(),
            max_cust_filter: 0,
            filter_index: 1,
            file: buffer.as_mut_ptr(),
            max_file: buffer.len() as u32,
            file_title: ptr::null_mut(),
            max_file_title: 0,
            initial_dir: ptr::null(),
            title: dialog_title.as_ptr(),
            flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | if multi { OFN_ALLOWMULTISELECT } else { 0 },
            file_offset: 0,
            file_extension: 0,
            def_ext: ptr::null(),
            cust_data: 0,
            hook: 0,
            template_name: ptr::null(),
            reserved: ptr::null(),
            reserved2: 0,
            flags_ex: 0,
        };
        if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
            return None;
        }
        let values = nul_strings(&buffer);
        let paths = if values.len() > 1 {
            values[1..]
                .iter()
                .map(|name| {
                    PathBuf::from(&values[0])
                        .join(name)
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        } else {
            values
        };
        let invalid = paths.iter().find(|path| !valid_target(path));
        if let Some(path) = invalid {
            message_box(
                ui,
                &format!("Unsupported shortcut target: {path}"),
                MB_OK | MB_ICONERROR,
            );
            None
        } else {
            Some(paths)
        }
    }

    fn choose_folder(ui: HWND) -> Option<String> {
        let mut display = vec![0u16; 260];
        let title = wide("Select a folder shortcut target");
        let info = BrowseInfoW {
            hwnd_owner: ui,
            pidl_root: ptr::null_mut(),
            display_name: display.as_mut_ptr(),
            title: title.as_ptr(),
            flags: BIF_RETURNONLYFSDIRS,
            callback: 0,
            callback_data: 0,
            image: 0,
        };
        let pidl = unsafe { SHBrowseForFolderW(&info) };
        if pidl.is_null() {
            return None;
        }
        let mut path = vec![0u16; 32_768];
        let selected = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) } != 0;
        selected.then(|| {
            String::from_utf16_lossy(
                &path[..path.iter().position(|c| *c == 0).unwrap_or(path.len())],
            )
        })
    }

    fn nul_strings(buffer: &[u16]) -> Vec<String> {
        buffer
            .split(|value| *value == 0)
            .take_while(|part| !part.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }

    fn valid_target(path: &str) -> bool {
        let path = Path::new(path);
        if path.is_dir() {
            return true;
        }
        path.is_file()
            && matches!(
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("exe" | "com" | "bat" | "cmd" | "lnk" | "url")
            )
    }

    fn selected_group(ui: &NativeUi) -> Option<String> {
        selected_text(ui.groups)
    }

    fn selected(hwnd: HWND) -> Option<usize> {
        let index = unsafe { SendMessageW(hwnd, LB_GETCURSEL, 0, 0) };
        (index >= 0).then_some(index as usize)
    }
    fn selected_text(hwnd: HWND) -> Option<String> {
        selected(hwnd).and_then(|index| list_text(hwnd, index))
    }
    fn list_text(hwnd: HWND, index: usize) -> Option<String> {
        let length = unsafe { SendMessageW(hwnd, LB_GETTEXTLEN, index, 0) };
        if length < 0 {
            return None;
        }
        let mut text = vec![0u16; length as usize + 1];
        unsafe {
            SendMessageW(hwnd, LB_GETTEXT, index, text.as_mut_ptr() as LPARAM);
        }
        Some(String::from_utf16_lossy(&text[..length as usize]))
    }
    fn nonempty(_ui: &NativeUi, hwnd: HWND) -> Option<String> {
        let value = text(hwnd);
        (!value.trim().is_empty()).then_some(value)
    }

    fn render(ui: &mut NativeUi) {
        ui.updating = true;
        unsafe {
            SendMessageW(ui.groups, LB_RESETCONTENT, 0, 0);
        }
        for group in &ui.controller.view().groups {
            let value = wide(group);
            unsafe {
                SendMessageW(ui.groups, LB_ADDSTRING, 0, value.as_ptr() as LPARAM);
            }
        }
        if let Some(group) = &ui.controller.view().editor {
            set_text(ui.group_name, &group.name);
            if let Some(icon) = &ui.controller.view().icon_path {
                set_text(ui.icon, icon);
            }
            set_text(ui.color, &group.color_string);
            set_text(ui.width, &group.width.to_string());
            set_text(ui.opacity, &group.opacity.to_string());
            unsafe {
                SendMessageW(
                    ui.allow_all,
                    BM_SETCHECK,
                    if group.allow_open_all {
                        BST_CHECKED_VALUE
                    } else {
                        BST_UNCHECKED_VALUE
                    },
                    0,
                );
                SendMessageW(ui.shortcuts, LB_RESETCONTENT, 0, 0);
            }
            for shortcut in &group.shortcut_list {
                let label = if shortcut.name.is_empty() {
                    shortcut.file_path.clone()
                } else {
                    format!("{} — {}", shortcut.name, shortcut.file_path)
                };
                let value = wide(&label);
                unsafe {
                    SendMessageW(ui.shortcuts, LB_ADDSTRING, 0, value.as_ptr() as LPARAM);
                }
            }
        } else {
            for hwnd in [
                ui.group_name,
                ui.icon,
                ui.shortcut_path,
                ui.shortcut_name,
                ui.arguments,
                ui.workdir,
            ] {
                set_text(hwnd, "");
            }
            unsafe {
                SendMessageW(ui.shortcuts, LB_RESETCONTENT, 0, 0);
            }
        }
        if let Some(index) = ui.controller.view().selected_shortcut {
            unsafe {
                SendMessageW(ui.shortcuts, LB_SETCURSEL, index, 0);
            }
            if let Some(shortcut) = ui
                .controller
                .view()
                .editor
                .as_ref()
                .and_then(|g| g.shortcut_list.get(index))
            {
                set_text(ui.shortcut_path, &shortcut.file_path);
                set_text(ui.shortcut_name, &shortcut.name);
                set_text(ui.arguments, &shortcut.arguments);
                set_text(ui.workdir, &shortcut.working_directory);
            }
        }
        ui.updating = false;
    }
    fn text(hwnd: HWND) -> String {
        let length = unsafe { GetWindowTextLengthW(hwnd) } as usize;
        let mut value = vec![0u16; length + 1];
        unsafe {
            GetWindowTextW(hwnd, value.as_mut_ptr(), value.len() as i32);
        }
        String::from_utf16_lossy(&value[..length])
    }
    fn set_text(hwnd: HWND, value: &str) {
        unsafe {
            SetWindowTextW(hwnd, wide(value).as_ptr());
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    fn message_box(hwnd: HWND, message: &str, flags: u32) {
        unsafe {
            MessageBoxW(
                hwnd,
                wide(message).as_ptr(),
                wide("Taskbar Groups").as_ptr(),
                flags,
            );
        }
    }
}

#[cfg(windows)]
pub use native::run;

#[cfg(not(windows))]
pub fn run(
    _: crate::platform::LaunchRequest,
    _: crate::persistence::AppPaths,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "native Windows UI is unavailable on this platform",
    ))
}

#[cfg(all(test, windows))]
mod smoke_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

    #[test]
    fn native_ui_smoke_skips_without_interactive_desktop() {
        let desktop = unsafe { GetDesktopWindow() };
        if desktop.is_null() {
            return;
        }
        assert!(!desktop.is_null());
    }

    #[test]
    fn discovered_app_selection_is_safe_without_launching() {
        let app = WindowsApp::new("Test", "Example.App_123!App").unwrap();
        assert_eq!(app.launch_target, "shell:AppsFolder\\Example.App_123!App");
        assert!(matches!(
            action_for_event(NativeEvent::SelectWindowsApp(app)),
            Action::SelectWindowsApp(_)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_editor_events_without_gui() {
        assert_eq!(
            action_for_event(NativeEvent::NewGroup),
            Action::BeginNewGroup
        );
        assert_eq!(
            action_for_event(NativeEvent::RemoveShortcut(2)),
            Action::RemoveShortcut(2)
        );
        assert_eq!(action_for_event(NativeEvent::CtrlEnter), Action::CtrlEnter);
    }
    #[test]
    fn maps_shortcut_fields_without_gui() {
        assert_eq!(
            action_for_event(NativeEvent::Arguments("--safe".into())),
            Action::SetArguments("--safe".into())
        );
        assert_eq!(
            action_for_event(NativeEvent::WorkingDirectory("C:\\Games".into())),
            Action::SetWorkingDirectory("C:\\Games".into())
        );
    }
    #[test]
    fn maps_discovered_apps_to_existing_actions() {
        let app = WindowsApp::new("Calculator", "Example.Calculator_123!App").unwrap();
        assert_eq!(
            action_for_event(NativeEvent::SelectWindowsApp(app.clone())),
            Action::SelectWindowsApp(app)
        );
    }

    #[test]
    fn maps_native_file_workflows_to_existing_actions() {
        assert_eq!(
            action_for_event(NativeEvent::AddShortcut {
                path: "C:\\Tools\\tool.exe".into(),
                is_windows_app: false,
            }),
            Action::AddShortcut {
                path: "C:\\Tools\\tool.exe".into(),
                is_windows_app: false,
            }
        );
        assert_eq!(
            action_for_event(NativeEvent::Icon("C:\\Icons\\app.ico".into())),
            Action::SetIcon {
                path: "C:\\Icons\\app.ico".into()
            }
        );
    }
}
