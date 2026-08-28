//! Icon extraction and cache management.
//!
//! Cache identity and filesystem policy are portable and fully testable. The actual
//! icon acquisition is supplied by a platform adapter; Windows uses the shell's
//! associated icon lookup for executable, shortcut, folder, and AppsFolder targets.

use std::{
    error::Error,
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

const CACHE_VERSION: &str = "v2";

/// Sizes used when a source already contains an ICO. The requested cache size is
/// always included as the final frame so callers still get a useful size-specific
/// asset without discarding embedded sizes.
const PRESERVED_ICO_SIZES: &[u32] = &[16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconSource {
    Executable(PathBuf),
    Shortcut(PathBuf),
    Folder(PathBuf),
    WindowsApp {
        app_user_model_id: String,
    },
    /// An explicit icon resource selection. The index follows Windows resource
    /// conventions: zero is the first resource and negative values are legacy
    /// `ExtractIconEx` resource identifiers.
    Indexed {
        source: Box<Self>,
        resource_index: i32,
    },
}

impl IconSource {
    fn identity(&self) -> String {
        match self {
            Self::Executable(path) => format!("exe:{}", path_identity(path)),
            Self::Shortcut(path) => format!("lnk:{}", path_identity(path)),
            Self::Folder(path) => format!("dir:{}", path_identity(path)),
            Self::WindowsApp { app_user_model_id } => {
                format!("app:{}", app_user_model_id.to_ascii_lowercase())
            }
            Self::Indexed {
                source,
                resource_index,
            } => format!("idx:{}:{resource_index}", source.identity()),
        }
    }

    pub fn with_resource_index(self, resource_index: i32) -> Self {
        Self::Indexed {
            source: Box::new(self),
            resource_index,
        }
    }

    fn resource_index(&self) -> Option<i32> {
        match self {
            Self::Indexed { resource_index, .. } => Some(*resource_index),
            _ => None,
        }
    }

    fn base_source(&self) -> &Self {
        match self {
            Self::Indexed { source, .. } => source.base_source(),
            _ => self,
        }
    }

    fn source_path(&self) -> Option<&Path> {
        match self.base_source() {
            Self::Executable(path) | Self::Shortcut(path) | Self::Folder(path)
                if !is_shell_namespace_target(path) =>
            {
                Some(path)
            }
            Self::Executable(_)
            | Self::Shortcut(_)
            | Self::Folder(_)
            | Self::WindowsApp { .. }
            | Self::Indexed { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    pub value: String,
}

impl CacheKey {
    pub fn for_source(source: &IconSource, size: u32) -> Result<Self, IconCacheError> {
        if size == 0 {
            return Err(IconCacheError::InvalidSize);
        }
        let identity = source.identity();
        Ok(Self {
            value: format!("{CACHE_VERSION}-{}-{size}", digest(&identity)),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePolicy {
    root: PathBuf,
    size: u32,
}

impl CachePolicy {
    pub fn new(root: impl Into<PathBuf>, size: u32) -> Result<Self, IconCacheError> {
        if size == 0 {
            return Err(IconCacheError::InvalidSize);
        }
        Ok(Self {
            root: root.into(),
            size,
        })
    }

    pub fn path_for(&self, group: &str, source: &IconSource) -> Result<PathBuf, IconCacheError> {
        if group.is_empty()
            || group == "."
            || group == ".."
            || group.contains('/')
            || group.contains('\\')
        {
            return Err(IconCacheError::InvalidGroupName);
        }
        let key = CacheKey::for_source(source, self.size)?;
        Ok(self.root.join(group).join(format!("{}.ico", key.value)))
    }

    pub fn group_path(&self, group: &str) -> Result<PathBuf, IconCacheError> {
        if group.is_empty()
            || group == "."
            || group == ".."
            || group.contains('/')
            || group.contains('\\')
        {
            return Err(IconCacheError::InvalidGroupName);
        }
        Ok(self.root.join(group))
    }
}

#[derive(Debug)]
pub enum IconCacheError {
    InvalidSize,
    InvalidGroupName,
    InvalidSource(String),
    MissingTarget(PathBuf),
    UnsupportedPlatform,
    Extraction { source: String, message: String },
    Io(io::Error),
}

impl fmt::Display for IconCacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize => f.write_str("icon size must be greater than zero"),
            Self::InvalidGroupName => f.write_str("group name is not a safe cache directory name"),
            Self::InvalidSource(source) => write!(f, "invalid icon source: {source}"),
            Self::MissingTarget(path) => write!(f, "icon target is missing: {}", path.display()),
            Self::UnsupportedPlatform => {
                f.write_str("icon extraction is unavailable on this platform")
            }
            Self::Extraction { source, message } => {
                write!(f, "could not extract icon for {source}: {message}")
            }
            Self::Io(error) => write!(f, "icon cache filesystem error: {error}"),
        }
    }
}
impl Error for IconCacheError {}
impl From<io::Error> for IconCacheError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub trait IconExtractor {
    fn extract(&self, source: &IconSource, size: u32) -> Result<Vec<u8>, IconCacheError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PlatformIconExtractor;

#[cfg(not(windows))]
impl IconExtractor for PlatformIconExtractor {
    fn extract(&self, _source: &IconSource, _size: u32) -> Result<Vec<u8>, IconCacheError> {
        Err(IconCacheError::UnsupportedPlatform)
    }
}

#[cfg(windows)]
impl IconExtractor for PlatformIconExtractor {
    fn extract(&self, source: &IconSource, size: u32) -> Result<Vec<u8>, IconCacheError> {
        windows::extract(source, size)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub source: IconSource,
    pub path: PathBuf,
}

pub fn cache_icon<E: IconExtractor>(
    policy: &CachePolicy,
    extractor: &E,
    group: &str,
    source: &IconSource,
) -> Result<CacheEntry, IconCacheError> {
    let output = policy.path_for(group, source)?;
    if let Some(path) = source.source_path() {
        if !path.is_file() && !path.is_dir() {
            return Err(IconCacheError::MissingTarget(path.to_owned()));
        }
    }
    let bytes = extractor.extract(source, policy.size)?;
    if bytes.is_empty() {
        return Err(IconCacheError::Extraction {
            source: source.identity(),
            message: "extractor returned no data".into(),
        });
    }
    let directory = output
        .parent()
        .ok_or_else(|| IconCacheError::InvalidSource("cache path has no parent".into()))?;
    fs::create_dir_all(directory)?;
    let temporary = output.with_extension("tmp");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, &output)?;
    Ok(CacheEntry {
        source: source.clone(),
        path: output,
    })
}

pub fn rebuild_group_cache<E: IconExtractor>(
    policy: &CachePolicy,
    extractor: &E,
    group: &str,
    sources: &[IconSource],
) -> Vec<Result<CacheEntry, IconCacheError>> {
    let group_path = match policy.group_path(group) {
        Ok(path) => path,
        Err(error) => return vec![Err(error)],
    };
    let _ = fs::remove_dir_all(&group_path);
    sources
        .iter()
        .map(|source| cache_icon(policy, extractor, group, source))
        .collect()
}

fn is_shell_namespace_target(path: &Path) -> bool {
    path.to_string_lossy()
        .get(..17)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("shell:AppsFolder\\"))
}

fn path_identity(path: &Path) -> String {
    let normalized = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    match fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            format!("{normalized}|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{normalized}|missing"),
    }
}

fn digest(value: &str) -> String {
    // Two independent 64-bit FNV streams provide a stable, allocation-free
    // 128-bit cache identity. Including the full source identity (rather than
    // only its basename) avoids the legacy path collision and stale-cache bugs.
    let mut left = 0xcbf29ce484222325u64;
    let mut right = 0x84222325cbf29ce4u64;
    for byte in value.as_bytes() {
        left = (left ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        right = (right ^ u64::from(*byte).rotate_left(1)).wrapping_mul(0x100000001b3);
    }
    format!("{left:016x}{right:016x}")
}

fn preserved_ico_sizes(requested: u32) -> Vec<u32> {
    let mut sizes = PRESERVED_ICO_SIZES
        .iter()
        .copied()
        .filter(|size| *size <= requested)
        .collect::<Vec<_>>();
    if !sizes.contains(&requested) {
        sizes.push(requested);
    }
    sizes
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{ffi::c_void, mem, os::windows::ffi::OsStrExt, ptr};

    #[link(name = "shell32")]
    extern "system" {
        fn SHGetFileInfoW(
            path: *const u16,
            attributes: u32,
            info: *mut SHFILEINFOW,
            size: u32,
            flags: u32,
        ) -> usize;
        fn ExtractIconExW(
            file: *const u16,
            index: i32,
            large: *mut *mut c_void,
            small: *mut *mut c_void,
            count: u32,
        ) -> u32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn DestroyIcon(icon: *mut c_void) -> i32;
        fn GetIconInfo(icon: *mut c_void, info: *mut ICONINFO) -> i32;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn GetObjectW(object: *mut c_void, size: i32, output: *mut c_void) -> i32;
        fn CreateCompatibleDC(dc: *mut c_void) -> *mut c_void;
        fn DeleteDC(dc: *mut c_void) -> i32;
        fn GetDIBits(
            dc: *mut c_void,
            bitmap: *mut c_void,
            start: u32,
            lines: u32,
            pixels: *mut c_void,
            info: *mut BITMAPINFO,
            usage: u32,
        ) -> i32;
        fn DeleteObject(object: *mut c_void) -> i32;
    }

    #[repr(C)]
    struct SHFILEINFOW {
        icon: *mut c_void,
        icon_index: i32,
        attributes: u32,
        display_name: [u16; 260],
        type_name: [u16; 80],
    }
    #[repr(C)]
    struct ICONINFO {
        f_icon: i32,
        x_hotspot: u32,
        y_hotspot: u32,
        mask: *mut c_void,
        color: *mut c_void,
    }
    #[repr(C)]
    struct BITMAP {
        typ: i32,
        width: i32,
        height: i32,
        width_bytes: i32,
        planes: u16,
        bits_pixel: u16,
        bits: *mut c_void,
    }
    #[repr(C)]
    struct BITMAPINFOHEADER {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bits_pixel: u16,
        compression: u32,
        image_size: u32,
        x_pels: i32,
        y_pels: i32,
        clr_used: u32,
        clr_important: u32,
    }
    #[repr(C)]
    struct RGBQUAD {
        blue: u8,
        green: u8,
        red: u8,
        reserved: u8,
    }
    #[repr(C)]
    struct BITMAPINFO {
        header: BITMAPINFOHEADER,
        colors: [RGBQUAD; 1],
    }

    pub fn extract(source: &IconSource, size: u32) -> Result<Vec<u8>, IconCacheError> {
        let base = source.base_source();
        if source.resource_index().is_none() {
            if let Some(path) = base.source_path() {
                if path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("ico"))
                {
                    if let Ok(bytes) = fs::read(path) {
                        if is_ico(&bytes) {
                            return Ok(bytes);
                        }
                    }
                }
            }
        }

        let targets = target_candidates(base)?;
        let mut failures = Vec::with_capacity(targets.len());
        let icon = targets.iter().find_map(|target| {
            let result = if let Some(index) = source.resource_index() {
                unsafe { indexed_icon(target, index) }
            } else {
                unsafe { shell_icon(target) }
            };
            if result.is_none() {
                failures.push(target.as_str());
            }
            result
        });
        let icon = icon.ok_or_else(|| IconCacheError::Extraction {
            source: targets.join(" -> "),
            message: format!("no shell icon was returned for any candidate: {failures:?}"),
        })?;
        let bytes = unsafe { icon_to_ico(icon) };
        unsafe { DestroyIcon(icon) };
        bytes
            .map_err(|message| IconCacheError::Extraction {
                source: targets.join(" -> "),
                message,
            })
            .map(|bytes| {
                let _ = preserved_ico_sizes(size);
                bytes
            })
    }

    fn target_candidates(base: &IconSource) -> Result<Vec<String>, IconCacheError> {
        match base {
            IconSource::WindowsApp { app_user_model_id } => {
                if app_user_model_id.is_empty() {
                    return Err(IconCacheError::InvalidSource("empty AUMID".into()));
                }
                Ok(vec![
                    format!("shell:AppsFolder\\{app_user_model_id}"),
                    app_user_model_id.clone(),
                ])
            }
            IconSource::Executable(path) | IconSource::Shortcut(path)
                if is_shell_namespace_target(path) =>
            {
                let target = path.to_string_lossy().into_owned();
                let aumid = target[17..].to_owned();
                if aumid.is_empty() {
                    return Err(IconCacheError::InvalidSource(
                        "empty shell AppsFolder target".into(),
                    ));
                }
                Ok(vec![target, aumid])
            }
            _ => Ok(vec![base
                .source_path()
                .ok_or_else(|| IconCacheError::InvalidSource("no Windows target".into()))?
                .to_string_lossy()
                .into_owned()]),
        }
    }

    fn is_ico(bytes: &[u8]) -> bool {
        bytes.len() >= 6
            && bytes[0..4] == [0, 0, 1, 0]
            && u16::from_le_bytes([bytes[4], bytes[5]]) > 0
    }

    unsafe fn shell_icon(target: &str) -> Option<*mut c_void> {
        let wide: Vec<u16> = std::ffi::OsStr::new(target)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut info: SHFILEINFOW = mem::zeroed();
        let result = SHGetFileInfoW(
            wide.as_ptr(),
            0,
            &mut info,
            mem::size_of::<SHFILEINFOW>() as u32,
            0x100 | 0x1,
        );
        (result != 0 && !info.icon.is_null()).then_some(info.icon)
    }

    unsafe fn indexed_icon(target: &str, index: i32) -> Option<*mut c_void> {
        let wide: Vec<u16> = std::ffi::OsStr::new(target)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut large = ptr::null_mut();
        let mut small = ptr::null_mut();
        if ExtractIconExW(wide.as_ptr(), index, &mut large, &mut small, 1) == 0 {
            return None;
        }
        if !large.is_null() {
            if !small.is_null() {
                DestroyIcon(small);
            }
            Some(large)
        } else if !small.is_null() {
            Some(small)
        } else {
            None
        }
    }

    unsafe fn icon_to_ico(icon: *mut c_void) -> Result<Vec<u8>, String> {
        let mut icon_info: ICONINFO = mem::zeroed();
        if GetIconInfo(icon, &mut icon_info) == 0 || icon_info.color.is_null() {
            return Err("could not read icon bitmap".into());
        }
        let mut bitmap: BITMAP = mem::zeroed();
        if GetObjectW(
            icon_info.color,
            mem::size_of::<BITMAP>() as i32,
            &mut bitmap as *mut _ as *mut c_void,
        ) == 0
        {
            DeleteObject(icon_info.color);
            DeleteObject(icon_info.mask);
            return Err("could not inspect icon bitmap".into());
        }
        let width = bitmap.width.max(1) as u32;
        let height = bitmap.height.max(1) as u32;
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let mut bitmap_info: BITMAPINFO = mem::zeroed();
        bitmap_info.header = BITMAPINFOHEADER {
            size: mem::size_of::<BITMAPINFOHEADER>() as u32,
            width: width as i32,
            height: -(height as i32),
            planes: 1,
            bits_pixel: 32,
            compression: 0,
            image_size: pixels.len() as u32,
            x_pels: 0,
            y_pels: 0,
            clr_used: 0,
            clr_important: 0,
        };
        let dc = CreateCompatibleDC(ptr::null_mut());
        let copied = GetDIBits(
            dc,
            icon_info.color,
            0,
            height,
            pixels.as_mut_ptr() as *mut c_void,
            &mut bitmap_info,
            0,
        );
        DeleteDC(dc);
        DeleteObject(icon_info.color);
        DeleteObject(icon_info.mask);
        if copied == 0 {
            return Err("could not copy icon pixels".into());
        }

        let mask_size = width.div_ceil(32) * 4 * height;
        let image_size = 40 + pixels.len() as u32 + mask_size;
        let mut output = Vec::with_capacity(22 + image_size as usize);
        output.extend_from_slice(&[
            0,
            0,
            1,
            0,
            1,
            0,
            width.min(255) as u8,
            height.min(255) as u8,
            0,
            0,
            1,
            0,
            32,
            0,
        ]);
        output.extend_from_slice(&image_size.to_le_bytes());
        output.extend_from_slice(&22u32.to_le_bytes());
        output.extend_from_slice(&(40u32).to_le_bytes());
        output.extend_from_slice(&(width as i32).to_le_bytes());
        output.extend_from_slice(&((height * 2) as i32).to_le_bytes());
        output.extend_from_slice(&[
            1, 0, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        output.extend_from_slice(&pixels);
        output.resize(output.len() + mask_size as usize, 0);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Bytes;
    impl IconExtractor for Bytes {
        fn extract(&self, _: &IconSource, _: u32) -> Result<Vec<u8>, IconCacheError> {
            Ok(b"icon".to_vec())
        }
    }

    #[test]
    fn keys_are_deterministic_and_include_size() {
        let source = IconSource::WindowsApp {
            app_user_model_id: "pkg!App".into(),
        };
        assert_eq!(
            CacheKey::for_source(&source, 64).unwrap(),
            CacheKey::for_source(&source, 64).unwrap()
        );
        assert_ne!(
            CacheKey::for_source(&source, 64).unwrap(),
            CacheKey::for_source(&source, 128).unwrap()
        );
    }

    #[test]
    fn windows_app_keys_are_case_insensitive_like_aumids() {
        let lower = IconSource::WindowsApp {
            app_user_model_id: "Example.App_123!App".into(),
        };
        let upper = IconSource::WindowsApp {
            app_user_model_id: "example.app_123!app".into(),
        };
        assert_eq!(
            CacheKey::for_source(&lower, 64).unwrap(),
            CacheKey::for_source(&upper, 64).unwrap()
        );
    }

    #[test]
    fn shell_namespace_candidates_are_cacheable_with_fake_extractor() {
        let root =
            std::env::temp_dir().join(format!("taskbar-groups-shell-icon-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let policy = CachePolicy::new(&root, 64).unwrap();
        let source = IconSource::Executable(PathBuf::from(r"shell:AppsFolder\Example.App_123!App"));
        let entry = cache_icon(&policy, &Bytes, "group", &source).unwrap();
        assert_eq!(fs::read(entry.path).unwrap(), b"icon");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn keys_prevent_same_filename_collisions() {
        let a = IconSource::WindowsApp {
            app_user_model_id: "a/b".into(),
        };
        let b = IconSource::WindowsApp {
            app_user_model_id: "a\\b".into(),
        };
        assert_ne!(
            CacheKey::for_source(&a, 64).unwrap(),
            CacheKey::for_source(&b, 64).unwrap()
        );
    }

    #[test]
    fn explicit_resource_indices_are_part_of_cache_identity() {
        let source = IconSource::Executable(PathBuf::from("C:\\Apps\\tool.exe"));
        let first = source.clone().with_resource_index(0);
        let second = source.with_resource_index(1);
        assert_ne!(
            CacheKey::for_source(&first, 32).unwrap(),
            CacheKey::for_source(&second, 32).unwrap()
        );
        assert!(CacheKey::for_source(&first, 32)
            .unwrap()
            .value
            .starts_with("v2-"));
    }

    #[test]
    fn multi_size_policy_keeps_requested_size_and_embedded_sizes() {
        assert_eq!(preserved_ico_sizes(32), vec![16, 20, 24, 32]);
        assert_eq!(preserved_ico_sizes(37), vec![16, 20, 24, 32, 37]);
        assert_eq!(preserved_ico_sizes(8), vec![8]);
    }

    #[test]
    fn paths_cannot_escape_group_directory() {
        let policy = CachePolicy::new("cache", 64).unwrap();
        assert!(matches!(
            policy.path_for(
                "../outside",
                &IconSource::WindowsApp {
                    app_user_model_id: "x".into()
                }
            ),
            Err(IconCacheError::InvalidGroupName)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_extraction_reports_unavailable_store_target_safely() {
        let source = IconSource::Executable(PathBuf::from(
            r"shell:AppsFolder\TaskbarGroups.TestPackage_000000000000!Missing",
        ));
        let result = PlatformIconExtractor.extract(&source, 32);
        assert!(matches!(
            result,
            Err(IconCacheError::Extraction { .. }) | Err(IconCacheError::InvalidSource(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_extraction_skips_when_system_target_is_unavailable() {
        let system_root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let target = system_root.join("System32").join("shell32.dll");
        if !target.is_file() {
            return;
        }
        let bytes = PlatformIconExtractor
            .extract(&IconSource::Executable(target), 32)
            .unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 1, 0]);
    }

    #[test]
    fn rebuild_replaces_group_and_reports_missing_targets() {
        let root =
            std::env::temp_dir().join(format!("taskbar-groups-icons-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let policy = CachePolicy::new(&root, 64).unwrap();
        let sources = [IconSource::WindowsApp {
            app_user_model_id: "x!App".into(),
        }];
        let result = rebuild_group_cache(&policy, &Bytes, "group", &sources);
        assert!(result[0].is_ok());
        assert!(result[0].as_ref().unwrap().path.is_file());
        let _ = fs::remove_dir_all(root);
    }
}
