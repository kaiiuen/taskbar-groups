//! Windows shell-link creation with explicit taskbar identity.
//!
//! The request and identity types are platform-neutral so callers can validate and
//! dry-run link changes without COM. The actual `.lnk` writer is Windows-only and
//! uses the ShellLink COM object directly, avoiding a second wrapper dependency.

use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

const LEGACY_GROUP_PREFIX: &str = "tjackenpacken.taskbarGroup.menu.";

/// The identity written to `System.AppUserModel.ID` in a shell link.
///
/// Legacy group IDs are intentionally kept behind this variant. A future migration
/// can add a new variant without changing the link request or conflating old IDs
/// with arbitrary Windows application identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppUserModelId {
    LegacyGroup { group_name: String },
    Explicit(String),
}

impl AppUserModelId {
    pub fn legacy_group(group_name: impl Into<String>) -> Result<Self, IdentityError> {
        let group_name = group_name.into();
        validate_text("group name", &group_name)?;
        Ok(Self::LegacyGroup { group_name })
    }

    pub fn explicit(value: impl Into<String>) -> Result<Self, IdentityError> {
        let value = value.into();
        validate_text("AppUserModelID", &value)?;
        Ok(Self::Explicit(value))
    }

    pub fn value(&self) -> String {
        match self {
            Self::LegacyGroup { group_name } => format!("{LEGACY_GROUP_PREFIX}{group_name}"),
            Self::Explicit(value) => value.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    Empty { field: &'static str },
    ContainsNul { field: &'static str },
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { field } => write!(f, "{field} must not be empty"),
            Self::ContainsNul { field } => write!(f, "{field} must not contain a NUL"),
        }
    }
}
impl Error for IdentityError {}

/// All data needed to create or replace one group `.lnk` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLinkRequest {
    pub link_path: PathBuf,
    pub target: PathBuf,
    pub arguments: String,
    pub working_directory: Option<PathBuf>,
    pub icon_path: Option<PathBuf>,
    pub icon_index: i32,
    pub description: String,
    pub app_user_model_id: AppUserModelId,
    /// Validate and report the operation without initializing COM or touching disk.
    pub dry_run: bool,
}

impl ShellLinkRequest {
    pub fn validate(&self) -> Result<(), ShellLinkError> {
        if self.link_path.as_os_str().is_empty() {
            return Err(ShellLinkError::InvalidRequest("link path is empty"));
        }
        if self
            .link_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.eq_ignore_ascii_case("lnk"))
            != Some(true)
        {
            return Err(ShellLinkError::InvalidRequest(
                "link path must have a .lnk extension",
            ));
        }
        if self.target.as_os_str().is_empty() {
            return Err(ShellLinkError::InvalidRequest("target path is empty"));
        }
        for (field, value) in [
            ("arguments", self.arguments.as_str()),
            ("description", self.description.as_str()),
        ] {
            validate_optional_text(field, value).map_err(ShellLinkError::Identity)?;
        }
        for (field, path) in [
            ("link path", Some(&self.link_path)),
            ("target path", Some(&self.target)),
            ("working directory", self.working_directory.as_ref()),
            ("icon path", self.icon_path.as_ref()),
        ] {
            if let Some(path) = path {
                if path.to_string_lossy().contains('\0') {
                    return Err(ShellLinkError::InvalidRequestOwned {
                        message: format!("{field} contains a NUL"),
                    });
                }
            }
        }
        // Resolving an icon is deliberately not part of this component, but the
        // index is still required to be a valid ShellLink icon resource index.
        if self.icon_index < 0 && self.icon_path.is_some() {
            return Err(ShellLinkError::InvalidRequest(
                "icon index must be non-negative",
            ));
        }
        validate_text("AppUserModelID", &self.app_user_model_id.value())
            .map_err(ShellLinkError::Identity)
    }

    pub fn for_group(
        group_name: impl Into<String>,
        link_path: impl Into<PathBuf>,
        target: impl Into<PathBuf>,
    ) -> Result<Self, ShellLinkError> {
        let group_name = group_name.into();
        Ok(Self {
            description: group_name.clone(),
            app_user_model_id: AppUserModelId::legacy_group(group_name)
                .map_err(ShellLinkError::Identity)?,
            link_path: link_path.into(),
            target: target.into(),
            arguments: String::new(),
            working_directory: None,
            icon_path: None,
            icon_index: 0,
            dry_run: false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLinkResult {
    pub link_path: PathBuf,
    pub app_user_model_id: String,
    pub written: bool,
}

#[derive(Debug)]
pub enum ShellLinkError {
    InvalidRequest(&'static str),
    InvalidRequestOwned { message: String },
    Identity(IdentityError),
    Io(std::io::Error),
    Com { operation: &'static str, code: i32 },
    UnsupportedPlatform,
}

impl fmt::Display for ShellLinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid shell-link request: {message}"),
            Self::InvalidRequestOwned { message } => {
                write!(f, "invalid shell-link request: {message}")
            }
            Self::Identity(error) => error.fmt(f),
            Self::Io(error) => write!(f, "shell-link filesystem error: {error}"),
            Self::Com { operation, code } => write!(
                f,
                "Windows COM {operation} failed with HRESULT 0x{:08x}",
                *code as u32
            ),
            Self::UnsupportedPlatform => {
                f.write_str("shell-link creation is only supported on Windows")
            }
        }
    }
}
impl Error for ShellLinkError {}
impl From<std::io::Error> for ShellLinkError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Create or replace a `.lnk`; dry-run requests only validate and return a plan.
pub fn create_or_update(request: &ShellLinkRequest) -> Result<ShellLinkResult, ShellLinkError> {
    request.validate()?;
    let result = ShellLinkResult {
        link_path: request.link_path.clone(),
        app_user_model_id: request.app_user_model_id.value(),
        written: !request.dry_run,
    };
    if request.dry_run {
        return Ok(result);
    }
    write_shell_link(request)?;
    Ok(result)
}

#[cfg(not(windows))]
fn write_shell_link(_request: &ShellLinkRequest) -> Result<(), ShellLinkError> {
    Err(ShellLinkError::UnsupportedPlatform)
}

#[cfg(windows)]
fn write_shell_link(request: &ShellLinkRequest) -> Result<(), ShellLinkError> {
    unsafe { com::write(request) }
}

fn validate_text(field: &'static str, value: &str) -> Result<(), IdentityError> {
    if value.is_empty() {
        return Err(IdentityError::Empty { field });
    }
    validate_optional_text(field, value)
}

fn validate_optional_text(field: &'static str, value: &str) -> Result<(), IdentityError> {
    if value.contains('\0') {
        return Err(IdentityError::ContainsNul { field });
    }
    Ok(())
}

#[cfg(windows)]
mod com {
    use super::*;
    use std::{ffi::c_void, mem, ptr};

    const S_OK: i32 = 0;
    const S_FALSE: i32 = 1;
    const RPC_E_CHANGED_MODE: i32 = -2147417850;
    const CLSCTX_INPROC_SERVER: u32 = 1;
    const VT_LPWSTR: u16 = 31;

    #[repr(C)]
    struct ComObject {
        vtable: *const usize,
    }
    #[repr(C)]
    struct PropVariant {
        vt: u16,
        reserved1: u16,
        reserved2: u16,
        reserved3: u16,
        value: *mut u16,
    }
    #[repr(C)]
    struct PropertyKey {
        fmtid: [u8; 16],
        pid: u32,
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
        fn CoUninitialize();
        fn CoCreateInstance(
            clsid: *const u8,
            outer: *mut c_void,
            context: u32,
            iid: *const u8,
            object: *mut *mut c_void,
        ) -> i32;

    }

    pub unsafe fn write(request: &ShellLinkRequest) -> Result<(), ShellLinkError> {
        let initialized = match CoInitializeEx(ptr::null_mut(), 2) {
            S_OK | S_FALSE => true,
            RPC_E_CHANGED_MODE => false,
            code => {
                return Err(ShellLinkError::Com {
                    operation: "initialization",
                    code,
                })
            }
        };
        let result = write_initialized(request);
        if initialized {
            CoUninitialize();
        }
        result
    }

    unsafe fn write_initialized(request: &ShellLinkRequest) -> Result<(), ShellLinkError> {
        // CLSID_ShellLink and IID_IShellLinkW, encoded in Windows GUID byte order.
        let clsid = [
            0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ];
        let iid = [
            0xF9, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ];
        let mut object = ptr::null_mut();
        check(
            CoCreateInstance(
                clsid.as_ptr(),
                ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                iid.as_ptr(),
                &mut object,
            ),
            "ShellLink creation",
        )?;
        let link = object as *mut ComObject;
        let result = populate_and_save(link, request);
        release(link);
        result
    }

    unsafe fn populate_and_save(
        link: *mut ComObject,
        request: &ShellLinkRequest,
    ) -> Result<(), ShellLinkError> {
        let target = wide_path(&request.target);
        call(link, 20, (target.as_ptr(),), "SetPath")?;
        let description = wide(&request.description);
        call(link, 7, (description.as_ptr(),), "SetDescription")?;
        let arguments = wide(&request.arguments);
        call(link, 10, (arguments.as_ptr(),), "SetArguments")?;
        if let Some(directory) = &request.working_directory {
            let directory = wide_path(directory);
            call(link, 8, (directory.as_ptr(),), "SetWorkingDirectory")?;
        }
        if let Some(icon) = &request.icon_path {
            let icon = wide_path(icon);
            call(
                link,
                14,
                (icon.as_ptr(), request.icon_index),
                "SetIconLocation",
            )?;
        }

        let mut store = ptr::null_mut();
        let property_iid = [
            0xEB, 0xD9, 0x6D, 0x88, 0x6D, 0x8C, 0xF2, 0x44, 0x8D, 0x02, 0xCD, 0xBA, 0x1D, 0xBD,
            0xCF, 0x99,
        ];
        query_interface(link, property_iid.as_ptr(), &mut store, "IPropertyStore")?;
        let store = store as *mut ComObject;
        let app_id = wide(&request.app_user_model_id.value());
        let mut variant = PropVariant {
            vt: VT_LPWSTR,
            reserved1: 0,
            reserved2: 0,
            reserved3: 0,
            value: app_id.as_ptr() as *mut u16,
        };
        let key = PropertyKey {
            fmtid: [
                0x55, 0x28, 0x4C, 0x9F, 0x79, 0x9F, 0x39, 0x4B, 0xA8, 0xD0, 0xE1, 0xD4, 0x2D, 0xE1,
                0xD5, 0xF3,
            ],
            pid: 5,
        };
        let property_result = call(store, 5, (&key, &mut variant), "Set AppUserModelID")
            .and_then(|_| call(store, 6, (), "Commit properties"));
        release(store);
        property_result?;

        let persist_iid = [
            0x0B, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ];
        let mut persist = ptr::null_mut();
        query_interface(link, persist_iid.as_ptr(), &mut persist, "IPersistFile")?;
        let persist = persist as *mut ComObject;
        let path = wide_path(&request.link_path);
        let result = call(persist, 6, (path.as_ptr(), 1i16), "Save link");
        release(persist);
        result
    }

    unsafe fn query_interface(
        object: *mut ComObject,
        iid: *const u8,
        result: *mut *mut c_void,
        operation: &'static str,
    ) -> Result<(), ShellLinkError> {
        let function: extern "system" fn(*mut ComObject, *const u8, *mut *mut c_void) -> i32 =
            mem::transmute((*object).vtable.add(0).read());
        check(function(object, iid, result), operation)
    }
    unsafe fn release(object: *mut ComObject) {
        let function: extern "system" fn(*mut ComObject) -> u32 =
            mem::transmute((*object).vtable.add(2).read());
        function(object);
    }
    unsafe fn call<A, R>(
        object: *mut ComObject,
        slot: usize,
        args: A,
        operation: &'static str,
    ) -> Result<(), ShellLinkError>
    where
        A: Call<R>,
    {
        args.call(object, slot, operation)
    }
    trait Call<R> {
        unsafe fn call(
            self,
            object: *mut ComObject,
            slot: usize,
            operation: &'static str,
        ) -> Result<(), ShellLinkError>;
    }
    impl Call<()> for () {
        unsafe fn call(
            self,
            object: *mut ComObject,
            slot: usize,
            operation: &'static str,
        ) -> Result<(), ShellLinkError> {
            let f: extern "system" fn(*mut ComObject) -> i32 =
                mem::transmute((*object).vtable.add(slot).read());
            check(f(object), operation)
        }
    }
    impl Call<()> for (*const u16,) {
        unsafe fn call(
            self,
            object: *mut ComObject,
            slot: usize,
            operation: &'static str,
        ) -> Result<(), ShellLinkError> {
            let f: extern "system" fn(*mut ComObject, *const u16) -> i32 =
                mem::transmute((*object).vtable.add(slot).read());
            check(f(object, self.0), operation)
        }
    }
    impl Call<()> for (*const u16, i32) {
        unsafe fn call(
            self,
            object: *mut ComObject,
            slot: usize,
            operation: &'static str,
        ) -> Result<(), ShellLinkError> {
            let f: extern "system" fn(*mut ComObject, *const u16, i32) -> i32 =
                mem::transmute((*object).vtable.add(slot).read());
            check(f(object, self.0, self.1), operation)
        }
    }
    impl Call<()> for (*const u16, i16) {
        unsafe fn call(
            self,
            object: *mut ComObject,
            slot: usize,
            operation: &'static str,
        ) -> Result<(), ShellLinkError> {
            let f: extern "system" fn(*mut ComObject, *const u16, i16) -> i32 =
                mem::transmute((*object).vtable.add(slot).read());
            check(f(object, self.0, self.1), operation)
        }
    }
    impl Call<()> for (&PropertyKey, &mut PropVariant) {
        unsafe fn call(
            self,
            object: *mut ComObject,
            slot: usize,
            operation: &'static str,
        ) -> Result<(), ShellLinkError> {
            let f: extern "system" fn(*mut ComObject, *const PropertyKey, *mut PropVariant) -> i32 =
                mem::transmute((*object).vtable.add(slot).read());
            check(f(object, self.0, self.1), operation)
        }
    }
    fn check(code: i32, operation: &'static str) -> Result<(), ShellLinkError> {
        if code >= 0 {
            Ok(())
        } else {
            Err(ShellLinkError::Com { operation, code })
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn wide_path(value: &Path) -> Vec<u16> {
        wide(&value.to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_group_identity_is_isolated_and_stable() {
        let id = AppUserModelId::legacy_group("Games").unwrap();
        assert_eq!(id.value(), "tjackenpacken.taskbarGroup.menu.Games");
        assert_eq!(
            AppUserModelId::explicit("future.publisher.games")
                .unwrap()
                .value(),
            "future.publisher.games"
        );
    }

    #[test]
    fn request_dry_run_reports_without_writing() {
        let mut request = ShellLinkRequest::for_group(
            "Games",
            PathBuf::from("out/Games.lnk"),
            PathBuf::from("app.exe"),
        )
        .unwrap();
        request.dry_run = true;
        let result = create_or_update(&request).unwrap();
        assert!(!result.written);
        assert_eq!(result.link_path, Path::new("out/Games.lnk"));
    }

    #[test]
    fn request_rejects_non_link_paths_and_nul_text() {
        let mut request = ShellLinkRequest::for_group("Games", "Games.txt", "app.exe").unwrap();
        assert!(matches!(
            request.validate(),
            Err(ShellLinkError::InvalidRequest(_))
        ));
        request.link_path = PathBuf::from("Games.lnk");
        request.arguments = "bad\0argument".to_owned();
        assert!(matches!(
            request.validate(),
            Err(ShellLinkError::Identity(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_writes_a_link_to_a_temporary_path() {
        let system_root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let target = system_root.join("System32").join("cmd.exe");
        if !target.is_file() {
            return;
        }
        let link_path = std::env::temp_dir().join(format!(
            "taskbar-groups-shell-link-{}.lnk",
            std::process::id()
        ));
        let mut request = ShellLinkRequest::for_group("Integration", &link_path, &target).unwrap();
        request.arguments = "/c exit 0".to_owned();
        request.working_directory = Some(system_root);
        request.description = "Integration shortcut".to_owned();
        request.icon_path = Some(target.clone());
        request.icon_index = 0;
        match create_or_update(&request) {
            Ok(result) => {
                assert!(result.written);
                assert!(link_path.is_file());
                let _ = std::fs::remove_file(link_path);
            }
            Err(ShellLinkError::Com { .. }) | Err(ShellLinkError::Io(_)) => {
                // COM registration or the temp directory may be unavailable in CI.
            }
            Err(error) => panic!("unexpected Windows shell-link error: {error}"),
        }
    }

    #[test]
    fn dry_run_accepts_icon_index_and_preserves_all_fields() {
        let request = ShellLinkRequest {
            link_path: PathBuf::from("group.lnk"),
            target: PathBuf::from("app.exe"),
            arguments: "Games".to_owned(),
            working_directory: Some(PathBuf::from("work")),
            icon_path: Some(PathBuf::from("icon.dll")),
            icon_index: 3,
            description: "Games shortcut".to_owned(),
            app_user_model_id: AppUserModelId::legacy_group("Games").unwrap(),
            dry_run: true,
        };
        let result = create_or_update(&request).unwrap();
        assert_eq!(
            result.app_user_model_id,
            "tjackenpacken.taskbarGroup.menu.Games"
        );
    }
}
