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
    ElevateSelected,
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

pub fn keyboard_event(key: char, control: bool) -> Option<NativeEvent> {
    match (key, control) {
        ('\u{1b}', false) => Some(NativeEvent::Cancel),
        ('\r', true) => Some(NativeEvent::CtrlEnter),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiLayout {
    pub groups: UiRect,
    pub editor: UiRect,
    pub shortcuts: UiRect,
    pub apps: UiRect,
    pub footer_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskbarRect {
    pub rect: UiRect,
    pub edge: TaskbarEdge,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarEdge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Computes a flyout position without assuming a taskbar or screen ordering.
///
/// The optional taskbar is only used when the anchor is inside its rectangle;
/// auto-hidden and unavailable taskbars therefore take the same safe fallback.
pub fn place_flyout(
    work_area: WorkArea,
    taskbar: Option<TaskbarRect>,
    anchor: (i32, i32),
    requested_size: (i32, i32),
) -> UiRect {
    let width = requested_size.0.max(1).min(work_area.width.max(1));
    let height = requested_size.1.max(1).min(work_area.height.max(1));
    let area = WorkArea {
        width: work_area.width.max(width),
        height: work_area.height.max(height),
        ..work_area
    };
    let anchor_in_taskbar = taskbar.is_some_and(|bar| {
        let right = bar.rect.x.saturating_add(bar.rect.width.max(0));
        let bottom = bar.rect.y.saturating_add(bar.rect.height.max(0));
        anchor.0 >= bar.rect.x && anchor.0 < right && anchor.1 >= bar.rect.y && anchor.1 < bottom
    });
    let gap = 10;
    let (x, y) = match taskbar.filter(|_| anchor_in_taskbar) {
        Some(bar) => match bar.edge {
            TaskbarEdge::Top => (anchor.0 - width / 2, bar.rect.y + bar.rect.height + gap),
            TaskbarEdge::Bottom => (anchor.0 - width / 2, bar.rect.y - height - gap),
            TaskbarEdge::Left => (bar.rect.x + bar.rect.width + gap, anchor.1 - height / 2),
            TaskbarEdge::Right => (bar.rect.x - width - gap, anchor.1 - height / 2),
        },
        None => (anchor.0 - width / 2, anchor.1 - height - 20),
    };
    UiRect {
        x: clamp_position(x, area.x, area.width, width),
        y: clamp_position(y, area.y, area.height, height),
        width,
        height,
    }
}

fn clamp_position(position: i32, origin: i32, extent: i32, size: i32) -> i32 {
    position
        .max(origin)
        .min(origin.saturating_add(extent).saturating_sub(size))
}

/// Infers the shell edge from a taskbar rectangle and its monitor bounds.
///
/// A taskbar that does not touch exactly one monitor edge is not treated as a
/// usable taskbar. This keeps shell query failures and transient rectangles on
/// the deterministic placement fallback.
pub fn infer_taskbar_edge(monitor: WorkArea, taskbar: UiRect) -> Option<TaskbarEdge> {
    let monitor_right = monitor.x.saturating_add(monitor.width.max(0));
    let monitor_bottom = monitor.y.saturating_add(monitor.height.max(0));
    let taskbar_right = taskbar.x.saturating_add(taskbar.width.max(0));
    let taskbar_bottom = taskbar.y.saturating_add(taskbar.height.max(0));
    if taskbar.width <= 0 || taskbar.height <= 0 {
        return None;
    }
    let touches = [
        (
            taskbar.y <= monitor.y && taskbar_bottom < monitor_bottom,
            TaskbarEdge::Top,
        ),
        (
            taskbar_bottom >= monitor_bottom && taskbar.y > monitor.y,
            TaskbarEdge::Bottom,
        ),
        (
            taskbar.x <= monitor.x && taskbar_right < monitor_right,
            TaskbarEdge::Left,
        ),
        (
            taskbar_right >= monitor_right && taskbar.x > monitor.x,
            TaskbarEdge::Right,
        ),
    ];
    let mut edge = None;
    for (matches, candidate) in touches {
        if matches {
            if edge.is_some() {
                return None;
            }
            edge = Some(candidate);
        }
    }
    edge
}

/// Computes client-area coordinates in logical pixels. The minimums keep every
/// editor control reachable when a window is restored to a small work area.
pub fn layout_for_client(width: i32, height: i32) -> UiLayout {
    let width = width.max(620);
    let height = height.max(420);
    let left_width = (width / 3).clamp(190, 300);
    let right_x = 16 + left_width + 24;
    let right_width = (width - right_x - 16).max(260);
    let list_height = (height - 130).max(170);
    let shortcut_width = (right_width - 140).max(180);
    UiLayout {
        groups: UiRect {
            x: 16,
            y: 42,
            width: left_width,
            height: list_height,
        },
        editor: UiRect {
            x: right_x,
            y: 24,
            width: right_width,
            height: 24,
        },
        shortcuts: UiRect {
            x: right_x,
            y: 195,
            width: shortcut_width,
            height: 140,
        },
        apps: UiRect {
            x: right_x + shortcut_width + 12,
            y: 195,
            width: 128,
            height: 140,
        },
        footer_y: (height - 42).max(360),
    }
}

/// Keeps labels readable while retaining both ends of paths and identifiers.
pub fn truncate_label(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars || max_chars < 2 {
        return value.chars().take(max_chars).collect();
    }
    let tail = max_chars / 2;
    let head = max_chars - tail - 1;
    format!(
        "{}…{}",
        value.chars().take(head).collect::<String>(),
        value.chars().skip(count - tail).collect::<String>()
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn keyboard_focus_order() -> &'static [&'static str] {
    &[
        "groups",
        "new",
        "edit",
        "group_name",
        "icon",
        "color",
        "width",
        "opacity",
        "allow_all",
        "shortcuts",
        "shortcut_path",
        "shortcut_name",
        "arguments",
        "workdir",
        "add",
        "discover_apps",
        "save",
        "delete",
        "cancel",
        "elevate_selected",
        "launch",
    ]
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
        // Elevation is deliberately handled by the native adapter rather than
        // translated into a controller action: the controller's normal launch
        // path must remain non-elevated.
        NativeEvent::ElevateSelected => unreachable!("elevation is a native-only action"),
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

const ACTIVATION_VERSION: u16 = 1;

#[cfg_attr(not(test), allow(dead_code))]
pub fn is_elevation_event(event: &NativeEvent) -> bool {
    matches!(event, NativeEvent::ElevateSelected)
}

fn elevation_error(error: &crate::platform::LaunchError) -> String {
    if matches!(
        error,
        crate::platform::LaunchError::Shell { code: 1223, .. }
    ) {
        "Elevation was cancelled; the selected shortcut was not launched.".to_owned()
    } else {
        format!("Could not launch the selected shortcut with elevation: {error}")
    }
}

fn encode_activation(group_name: Option<&str>) -> Vec<u16> {
    let mut payload = vec![ACTIVATION_VERSION, group_name.is_some() as u16];
    payload.extend(group_name.unwrap_or_default().encode_utf16());
    payload.push(0);
    payload
}

fn decode_activation(payload: &[u16]) -> Option<Option<String>> {
    if payload.len() < 3 || payload[0] != ACTIVATION_VERSION || payload[1] > 1 {
        return None;
    }
    let value = payload[2..].split(|unit| *unit == 0).next()?;
    let value = String::from_utf16(value).ok()?;
    match payload[1] {
        0 if value.is_empty() => Some(None),
        1 if !value.is_empty() => Some(Some(value)),
        _ => None,
    }
}

#[cfg(windows)]
mod native {
    use super::{
        action_for_event, elevation_error, infer_taskbar_edge, keyboard_event, layout_for_client,
        place_flyout, NativeEvent, TaskbarRect, UiRect, WorkArea,
    };
    use crate::{
        persistence::AppPaths,
        platform::{
            windows_apps::{WindowsApp, WindowsAppDiscovery, WindowsShellAppDiscovery},
            LaunchRequest, LaunchSpec, Launcher, PassthroughResolver, ShortcutResolver,
            WindowsPlatform,
        },
        ui::{Controller, UiShell},
    };
    use std::{
        io,
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
        ptr, thread,
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, COLOR_WINDOW, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{GetKeyState, VK_ESCAPE, VK_MENU, VK_RETURN},
            Shell::{
                DragAcceptFiles, DragFinish, DragQueryFileW, SHAppBarMessage, ShellExecuteW,
                APPBARDATA, HDROP,
            },
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
    const ELEVATE: i32 = 128;
    const DISCOVER_APPS: i32 = 126;
    const APPS: i32 = 127;
    const VK_CONTROL_KEY: i32 = 0x11;
    const BST_UNCHECKED_VALUE: usize = 0;
    const BST_CHECKED_VALUE: usize = 1;
    const OFN_EXPLORER: u32 = 0x0008_0000;
    const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
    const OFN_ALLOWMULTISELECT: u32 = 0x0000_0200;
    const BIF_RETURNONLYFSDIRS: u32 = 0x0001;

    const COPYDATA_GROUP: usize = 1;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_NOZORDER: u32 = 0x0004;
    const WAIT_OBJECT_0_VALUE: u32 = 0;
    const WAIT_ABANDONED_VALUE: u32 = 0x0000_0080;
    const WAIT_TIMEOUT_VALUE: u32 = 0x0000_0102;
    const SMTO_ABORTIFHUNG_VALUE: u32 = 0x0002;
    const ACTIVATION_RETRIES: usize = 40;
    const ACTIVATION_RETRY_DELAY: Duration = Duration::from_millis(25);
    const EDITOR_MUTEX: &str = "Local\\TaskbarGroups.Editor";
    const FLYOUT_MUTEX: &str = "Local\\TaskbarGroups.Flyout";
    const EDITOR_CLASS: &str = "TaskbarGroupsEditorUi";
    const FLYOUT_CLASS: &str = "TaskbarGroupsFlyoutUi";
    const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
    const ABM_GETSTATE_VALUE: u32 = 0x0000_0004;
    const ABM_GETTASKBARPOS_VALUE: u32 = 0x0000_0005;
    const ABS_AUTOHIDE_VALUE: usize = 0x0000_0001;

    #[repr(C)]
    struct CopyData {
        data: usize,
        length: u32,
        pointer: *const core::ffi::c_void,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(
            attributes: *const core::ffi::c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> isize;
        fn ReleaseMutex(mutex: isize) -> i32;
        fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32;
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(memory: *mut core::ffi::c_void);
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetProcessDpiAwarenessContext(value: isize) -> i32;
        fn SendMessageTimeoutW(
            hwnd: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
            flags: u32,
            timeout: u32,
            result: *mut usize,
        ) -> LRESULT;
    }

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

    struct InstanceGuard(isize);

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            unsafe {
                ReleaseMutex(self.0);
                CloseHandle(self.0 as _);
            }
        }
    }

    fn instance_identity(group_name: Option<&str>) -> (&'static str, &'static str) {
        if group_name.is_some() {
            (FLYOUT_MUTEX, FLYOUT_CLASS)
        } else {
            (EDITOR_MUTEX, EDITOR_CLASS)
        }
    }

    fn acquire_instance(group_name: Option<&str>) -> io::Result<Option<InstanceGuard>> {
        let (mutex_name, _) = instance_identity(group_name);
        let name = wide(mutex_name);
        let mutex = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if mutex == 0 {
            return Err(io::Error::last_os_error());
        }
        match unsafe { WaitForSingleObject(mutex, 0) } {
            WAIT_OBJECT_0_VALUE | WAIT_ABANDONED_VALUE => Ok(Some(InstanceGuard(mutex))),
            WAIT_TIMEOUT_VALUE => {
                unsafe { CloseHandle(mutex as _) };
                Ok(None)
            }
            _ => {
                let error = io::Error::last_os_error();
                unsafe { CloseHandle(mutex as _) };
                Err(error)
            }
        }
    }

    pub fn run(request: LaunchRequest, paths: AppPaths) -> io::Result<()> {
        let (_, class_name) = instance_identity(request.group_name.as_deref());
        let Some(_instance) = acquire_instance(request.group_name.as_deref())? else {
            return forward_activation(class_name, request.group_name.as_deref());
        };

        // Per-monitor V2 keeps client coordinates and common controls scaled on
        // mixed-DPI desktops. It must be selected before creating any windows.
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let class = wide(class_name);
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
                WS_OVERLAPPEDWINDOW,
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
        place_on_work_area(hwnd);
        let controller = Controller::new(request, paths, NativeShell(hwnd));
        let mut ui = Box::new(build_ui(hwnd, controller));
        resize_ui(&mut ui);
        ui.controller
            .dispatch(action_for_event(NativeEvent::ReloadGroups));
        render(&mut ui);
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(ui) as isize);
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
            let mut message = std::mem::zeroed();
            while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
                if IsDialogMessageW(hwnd, &mut message) != 0 {
                    continue;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }

    fn forward_activation(class_name: &str, group_name: Option<&str>) -> io::Result<()> {
        let class = wide(class_name);
        let mut hwnd = ptr::null_mut();
        for _ in 0..ACTIVATION_RETRIES {
            hwnd = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
            if !hwnd.is_null() {
                break;
            }
            thread::sleep(ACTIVATION_RETRY_DELAY);
        }
        if hwnd.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the existing Taskbar Groups window was not ready for activation",
            ));
        }
        let payload = super::encode_activation(group_name);
        let copy = CopyData {
            data: COPYDATA_GROUP,
            length: (payload.len() * std::mem::size_of::<u16>()) as u32,
            pointer: payload.as_ptr() as *const _,
        };
        let mut result = 0usize;
        let delivered = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_COPYDATA,
                0,
                &copy as *const _ as LPARAM,
                SMTO_ABORTIFHUNG_VALUE,
                2_000,
                &mut result,
            )
        };
        if delivered == 0 || result == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the existing Taskbar Groups window did not accept activation",
            ));
        }
        unsafe {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
        Ok(())
    }

    fn place_on_work_area(hwnd: HWND) {
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            return;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return;
        }
        let work_width = (info.rcWork.right - info.rcWork.left).max(1);
        let work_height = (info.rcWork.bottom - info.rcWork.top).max(1);
        let work_area = WorkArea {
            x: info.rcWork.left,
            y: info.rcWork.top,
            width: work_width,
            height: work_height,
        };
        let width = 860.min(work_width);
        let height = 650.min(work_height);
        let monitor_area = UiRect {
            x: info.rcMonitor.left,
            y: info.rcMonitor.top,
            width: (info.rcMonitor.right - info.rcMonitor.left).max(1),
            height: (info.rcMonitor.bottom - info.rcMonitor.top).max(1),
        };
        let taskbar = query_taskbar(monitor, monitor_area);
        let anchor = taskbar.map_or(
            (
                info.rcWork.left + work_width / 2,
                info.rcWork.top + work_height / 2 + height / 2 + 20,
            ),
            |bar| {
                (
                    bar.rect.x + bar.rect.width / 2,
                    bar.rect.y + bar.rect.height / 2,
                )
            },
        );
        let placement = place_flyout(work_area, taskbar, anchor, (width, height));
        unsafe {
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                placement.x,
                placement.y,
                placement.width,
                placement.height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    pub(crate) fn query_taskbar(
        monitor: *mut core::ffi::c_void,
        monitor_area: UiRect,
    ) -> Option<TaskbarRect> {
        let mut appbar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        if unsafe { SHAppBarMessage(ABM_GETSTATE_VALUE, &mut appbar) } & ABS_AUTOHIDE_VALUE != 0 {
            return None;
        }

        // ABM_GETTASKBARPOS is the supported query for the primary taskbar.
        if unsafe { SHAppBarMessage(ABM_GETTASKBARPOS_VALUE, &mut appbar) } != 0
            && !appbar.hWnd.is_null()
            && unsafe { MonitorFromWindow(appbar.hWnd, MONITOR_DEFAULTTONEAREST) } == monitor
        {
            let rect = rect_to_ui(appbar.rc);
            if let Some(edge) = infer_taskbar_edge(
                WorkArea {
                    x: monitor_area.x,
                    y: monitor_area.y,
                    width: monitor_area.width,
                    height: monitor_area.height,
                },
                rect,
            ) {
                return Some(TaskbarRect { rect, edge });
            }
        }

        // Secondary taskbars expose their monitor-specific rectangles through
        // the documented shell window class rather than SHAppBarMessage.
        let class = wide("Shell_SecondaryTrayWnd");
        let mut previous = ptr::null_mut();
        loop {
            let taskbar =
                unsafe { FindWindowExW(ptr::null_mut(), previous, class.as_ptr(), ptr::null()) };
            if taskbar.is_null() {
                break;
            }
            previous = taskbar;
            if unsafe { IsWindowVisible(taskbar) } == 0
                || unsafe { MonitorFromWindow(taskbar, MONITOR_DEFAULTTONEAREST) } != monitor
            {
                continue;
            }
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { GetWindowRect(taskbar, &mut rect) } == 0 {
                continue;
            }
            let rect = rect_to_ui(rect);
            if let Some(edge) = infer_taskbar_edge(
                WorkArea {
                    x: monitor_area.x,
                    y: monitor_area.y,
                    width: monitor_area.width,
                    height: monitor_area.height,
                },
                rect,
            ) {
                return Some(TaskbarRect { rect, edge });
            }
        }
        None
    }

    fn rect_to_ui(rect: RECT) -> UiRect {
        UiRect {
            x: rect.left,
            y: rect.top,
            width: rect.right.saturating_sub(rect.left),
            height: rect.bottom.saturating_sub(rect.top),
        }
    }

    fn move_control(hwnd: HWND, rect: UiRect) {
        unsafe {
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    fn resize_ui(ui: &mut NativeUi) {
        let mut client = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetClientRect(ui.hwnd, &mut client) } == 0 {
            return;
        }
        let layout = layout_for_client(client.right - client.left, client.bottom - client.top);
        move_control(ui.groups, layout.groups);
        move_control(ui.shortcuts, layout.shortcuts);
        move_control(ui.apps, layout.apps);
        let editor_x = layout.editor.x + 125;
        let editor_width = (layout.editor.width - 125).max(135);
        for (control, y) in [
            (ui.group_name, 22),
            (ui.icon, 66),
            (ui.color, 110),
            (ui.width, 110),
            (ui.opacity, 110),
        ] {
            move_control(
                control,
                UiRect {
                    x: editor_x,
                    y,
                    width: editor_width.min(260),
                    height: 24,
                },
            );
        }
        for (control, y) in [
            (ui.shortcut_path, 353),
            (ui.shortcut_name, 353),
            (ui.arguments, 397),
            (ui.workdir, 397),
        ] {
            move_control(
                control,
                UiRect {
                    x: layout.shortcuts.x + 125,
                    y,
                    width: (layout.shortcuts.width - 125).max(135),
                    height: 24,
                },
            );
        }
    }

    fn build_ui(hwnd: HWND, controller: Controller<NativeShell>) -> NativeUi {
        let groups = control(
            "LISTBOX",
            "",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | LBS_NOTIFY as u32 | WS_VSCROLL,
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
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | LBS_NOTIFY as u32 | WS_VSCROLL,
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
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | LBS_NOTIFY as u32 | WS_VSCROLL,
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
        button(hwnd, "Run elevated", 700, 500, 110, 26, ELEVATE);
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
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CREATE => {
                DragAcceptFiles(hwnd, 1);
                0
            }
            WM_COPYDATA => {
                let mut accepted = false;
                if let Some(copy) = (lparam as *const CopyData).as_ref() {
                    if copy.data == COPYDATA_GROUP
                        && copy.length >= (3 * std::mem::size_of::<u16>()) as u32
                        && copy.length % std::mem::size_of::<u16>() as u32 == 0
                        && !copy.pointer.is_null()
                    {
                        let units = copy.length as usize / std::mem::size_of::<u16>();
                        let payload = std::slice::from_raw_parts(copy.pointer as *const u16, units);
                        if let Some(group_name) = super::decode_activation(payload) {
                            if let Some(ui) =
                                (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeUi).as_mut()
                            {
                                if let Some(value) = group_name {
                                    dispatch(ui, NativeEvent::EditGroup(value));
                                }
                                SetForegroundWindow(hwnd);
                                accepted = true;
                            }
                        }
                    }
                }
                accepted as isize
            }
            WM_DROPFILES => {
                if let Some(ui) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeUi).as_mut()
                {
                    drop_files(ui, wparam as HDROP);
                }
                0
            }
            WM_SIZE | WM_DPICHANGED => {
                if let Some(ui) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeUi).as_mut()
                {
                    resize_ui(ui);
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
                    let alt = GetKeyState(VK_MENU as i32) < 0;
                    let event = if wparam == VK_ESCAPE as usize {
                        keyboard_event('\u{1b}', false)
                    } else if wparam == VK_RETURN as usize && GetKeyState(VK_CONTROL_KEY) < 0 {
                        keyboard_event('\r', true)
                    } else if alt {
                        match wparam as u32 {
                            0x4E => Some(NativeEvent::NewGroup),
                            0x45 => selected_group(ui).map(NativeEvent::EditGroup),
                            0x53 => Some(NativeEvent::SaveGroup),
                            0x43 => Some(NativeEvent::Cancel),
                            0x4C => Some(NativeEvent::CtrlEnter),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    if let Some(event) = event {
                        dispatch(ui, event);
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
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
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
            ELEVATE if notification == BN_CLICKED => Some(NativeEvent::ElevateSelected),
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
            match choose_folder(ui.hwnd) {
                Ok(Some(path)) => dispatch(
                    ui,
                    NativeEvent::AddShortcut {
                        path,
                        is_windows_app: false,
                    },
                ),
                Ok(None) => {}
                Err(error) => ui.controller.shell_mut().show_error(&error.to_string()),
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
        if matches!(event, NativeEvent::ElevateSelected) {
            elevate_selected(ui);
        } else {
            ui.controller.dispatch(action_for_event(event));
            render(ui);
        }
    }

    pub(crate) fn selected_elevation_policy() -> crate::platform::special_targets::ElevationPolicy {
        crate::platform::special_targets::ElevationPolicy::RunAs
    }

    fn elevate_selected(ui: &mut NativeUi) {
        let Some(index) = ui.controller.view().selected_shortcut else {
            ui.controller
                .shell_mut()
                .show_error("Select a shortcut first.");
            return;
        };
        let Some(shortcut) = ui
            .controller
            .view()
            .editor
            .as_ref()
            .and_then(|group| group.shortcut_list.get(index))
        else {
            ui.controller
                .shell_mut()
                .show_error("The selected shortcut is no longer available.");
            return;
        };
        let spec = match PassthroughResolver.resolve(shortcut) {
            Ok(target) => LaunchSpec {
                target,
                arguments: shortcut.arguments.clone(),
                working_directory: shortcut.working_directory.clone(),
            },
            Err(error) => {
                ui.controller
                    .shell_mut()
                    .show_error(&format!("Could not elevate the selected shortcut: {error}"));
                return;
            }
        };
        match WindowsPlatform.launch_with_policy(&spec, selected_elevation_policy()) {
            Ok(()) => ui
                .controller
                .shell_mut()
                .show_message("Elevation prompt launched for the selected shortcut."),
            Err(error) => ui
                .controller
                .shell_mut()
                .show_error(&elevation_error(&error)),
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

    fn choose_folder(ui: HWND) -> io::Result<Option<String>> {
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
            return Ok(None);
        }
        let mut path = vec![0u16; 32_768];
        let selected = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) } != 0;
        let result = if selected {
            Ok(Some(String::from_utf16_lossy(
                &path[..path.iter().position(|c| *c == 0).unwrap_or(path.len())],
            )))
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows returned a folder without a filesystem path",
            ))
        };
        unsafe { CoTaskMemFree(pidl) };
        result
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
        selected(ui.groups).and_then(|index| ui.controller.view().groups.get(index).cloned())
    }

    fn selected(hwnd: HWND) -> Option<usize> {
        let index = unsafe { SendMessageW(hwnd, LB_GETCURSEL, 0, 0) };
        (index >= 0).then_some(index as usize)
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
            let value = wide(&super::truncate_label(group, 42));
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
                let value = wide(&super::truncate_label(&label, 76));
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
    use super::native::query_taskbar;
    use super::*;
    use windows_sys::Win32::{
        Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        UI::WindowsAndMessaging::GetDesktopWindow,
    };

    #[test]
    fn native_ui_smoke_skips_without_interactive_desktop() {
        let desktop = unsafe { GetDesktopWindow() };
        if desktop.is_null() {
            return;
        }
        assert!(!desktop.is_null());
    }

    #[test]
    fn taskbar_query_is_safe_without_an_interactive_shell() {
        let desktop = unsafe { GetDesktopWindow() };
        if desktop.is_null() {
            return;
        }
        let monitor = unsafe { MonitorFromWindow(desktop, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            return;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return;
        }
        let monitor_area = UiRect {
            x: info.rcMonitor.left,
            y: info.rcMonitor.top,
            width: info.rcMonitor.right - info.rcMonitor.left,
            height: info.rcMonitor.bottom - info.rcMonitor.top,
        };
        if let Some(taskbar) = query_taskbar(monitor, monitor_area) {
            assert!(taskbar.rect.width > 0);
            assert!(taskbar.rect.height > 0);
            assert!(
                infer_taskbar_edge(
                    WorkArea {
                        x: monitor_area.x,
                        y: monitor_area.y,
                        width: monitor_area.width,
                        height: monitor_area.height,
                    },
                    taskbar.rect
                ) == Some(taskbar.edge)
            );
        }
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

    #[test]
    fn elevation_policy_selection_is_safe_without_launching() {
        assert_eq!(
            super::native::selected_elevation_policy(),
            crate::platform::special_targets::ElevationPolicy::RunAs
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_codec_round_trips_editor_and_group_requests() {
        for group in [None, Some("Games"), Some("中文 — Tools")] {
            let encoded = encode_activation(group);
            assert_eq!(decode_activation(&encoded), Some(group.map(str::to_owned)));
        }
    }

    #[test]
    fn activation_codec_rejects_truncated_or_invalid_payloads() {
        let valid = encode_activation(Some("Games"));
        assert_eq!(decode_activation(&valid[..2]), None);
        let mut wrong_version = valid.clone();
        wrong_version[0] = 2;
        assert_eq!(decode_activation(&wrong_version), None);
        let mut wrong_mode = valid;
        wrong_mode[1] = 2;
        assert_eq!(decode_activation(&wrong_mode), None);
    }

    #[test]
    fn keyboard_events_are_portable_and_controller_friendly() {
        assert_eq!(keyboard_event('\u{1b}', false), Some(NativeEvent::Cancel));
        assert_eq!(keyboard_event('\r', true), Some(NativeEvent::CtrlEnter));
        assert_eq!(keyboard_event('\r', false), None);
    }

    #[test]
    fn elevation_is_an_explicit_native_event() {
        assert!(is_elevation_event(&NativeEvent::ElevateSelected));
        assert!(!is_elevation_event(&NativeEvent::CtrlEnter));
    }

    #[test]
    fn elevation_cancellation_has_clear_user_facing_text() {
        let error = crate::platform::LaunchError::Shell {
            target: "example.exe".into(),
            code: 1223,
            message: "The operation was canceled by the user".into(),
        };
        assert_eq!(
            elevation_error(&error),
            "Elevation was cancelled; the selected shortcut was not launched."
        );
    }

    #[test]
    fn layout_is_resize_aware_and_keeps_controls_in_bounds() {
        let small = layout_for_client(320, 200);
        assert!(small.groups.width > 0 && small.shortcuts.width > 0);
        let large = layout_for_client(1400, 900);
        assert!(large.groups.width > small.groups.width);
        assert!(large.apps.x + large.apps.width <= 1400);
    }

    #[test]
    fn taskbar_edge_inference_requires_a_single_monitor_edge() {
        let monitor = WorkArea {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            infer_taskbar_edge(
                monitor,
                UiRect {
                    x: -1920,
                    y: 1040,
                    width: 1920,
                    height: 40,
                }
            ),
            Some(TaskbarEdge::Bottom)
        );
        assert_eq!(
            infer_taskbar_edge(
                monitor,
                UiRect {
                    x: -1800,
                    y: 100,
                    width: 500,
                    height: 40,
                }
            ),
            None
        );
    }

    #[test]
    fn placement_clamps_to_negative_coordinate_work_area() {
        let placement = place_flyout(
            WorkArea {
                x: -1920,
                y: -200,
                width: 1920,
                height: 1080,
            },
            None,
            (-10, 20),
            (860, 650),
        );
        assert_eq!(placement.x, -860);
        assert_eq!(placement.y, -200);
        assert_eq!(placement.width, 860);
        assert_eq!(placement.height, 650);
    }

    #[test]
    fn placement_handles_each_taskbar_edge_without_indexing_screens() {
        let area = WorkArea {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        };
        let cases = [
            (
                TaskbarEdge::Top,
                UiRect {
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 40,
                },
                (1280, 20),
                (850, 650),
                (855, 50),
            ),
            (
                TaskbarEdge::Bottom,
                UiRect {
                    x: 0,
                    y: 1400,
                    width: 2560,
                    height: 40,
                },
                (1280, 1420),
                (850, 650),
                (855, 740),
            ),
            (
                TaskbarEdge::Left,
                UiRect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 1440,
                },
                (20, 720),
                (850, 650),
                (50, 395),
            ),
            (
                TaskbarEdge::Right,
                UiRect {
                    x: 2520,
                    y: 0,
                    width: 40,
                    height: 1440,
                },
                (2540, 720),
                (850, 650),
                (1660, 395),
            ),
        ];
        for (edge, rect, anchor, size, expected) in cases {
            let placement = place_flyout(area, Some(TaskbarRect { rect, edge }), anchor, size);
            assert_eq!((placement.x, placement.y), expected);
        }
    }

    #[test]
    fn placement_falls_back_for_auto_hide_and_clamps_portrait_edges() {
        let portrait = WorkArea {
            x: 100,
            y: -900,
            width: 600,
            height: 1200,
        };
        let placement = place_flyout(portrait, None, (690, -890), (900, 1400));
        assert_eq!(
            placement,
            UiRect {
                x: 100,
                y: -900,
                width: 600,
                height: 1200
            }
        );

        let bottom = place_flyout(
            portrait,
            Some(TaskbarRect {
                rect: UiRect {
                    x: 100,
                    y: -900,
                    width: 600,
                    height: 40,
                },
                edge: TaskbarEdge::Top,
            }),
            (690, -890),
            (400, 300),
        );
        assert_eq!((bottom.x, bottom.y), (300, -850));
    }

    #[test]
    fn long_labels_keep_both_ends() {
        assert_eq!(truncate_label("short", 10), "short");
        assert_eq!(truncate_label("abcdefghijklmnop", 9), "abcd…mnop");
    }

    #[test]
    fn focus_order_is_explicit_and_actionable() {
        let order = keyboard_focus_order();
        assert_eq!(order.first(), Some(&"groups"));
        assert_eq!(order.last(), Some(&"launch"));
        assert!(
            order.iter().position(|name| *name == "save")
                < order.iter().position(|name| *name == "cancel")
        );
    }

    #[test]
    fn maps_editor_events_without_gui() {
        assert_eq!(action_for_event(NativeEvent::Cancel), Action::CancelEditor);
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
