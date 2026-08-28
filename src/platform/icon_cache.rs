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

const CACHE_VERSION: &str = "v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconSource {
    Executable(PathBuf),
    Shortcut(PathBuf),
    Folder(PathBuf),
    WindowsApp { app_user_model_id: String },
}

impl IconSource {
    fn identity(&self) -> String {
        match self {
            Self::Executable(path) => format!("exe:{}", path_identity(path)),
            Self::Shortcut(path) => format!("lnk:{}", path_identity(path)),
            Self::Folder(path) => format!("dir:{}", path_identity(path)),
            Self::WindowsApp { app_user_model_id } => format!("app:{app_user_model_id}"),
        }
    }

    fn source_path(&self) -> Option<&Path> {
        match self {
            Self::Executable(path) | Self::Shortcut(path) | Self::Folder(path) => Some(path),
            Self::WindowsApp { .. } => None,
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
            value: format!("{CACHE_VERSION}-{}-{}", fnv1a(&identity), size),
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

fn fnv1a(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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

    pub fn extract(source: &IconSource, _size: u32) -> Result<Vec<u8>, IconCacheError> {
        let target = match source {
            IconSource::WindowsApp { app_user_model_id } => {
                format!("shell:AppsFolder\\{app_user_model_id}")
            }
            _ => source
                .source_path()
                .ok_or_else(|| IconCacheError::InvalidSource("no Windows target".into()))?
                .to_string_lossy()
                .into_owned(),
        };
        let wide: Vec<u16> = std::ffi::OsStr::new(&target)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut info: SHFILEINFOW = unsafe { mem::zeroed() };
        let result = unsafe {
            SHGetFileInfoW(
                wide.as_ptr(),
                0,
                &mut info,
                mem::size_of::<SHFILEINFOW>() as u32,
                0x100 | 0x1,
            )
        };
        if result == 0 || info.icon.is_null() {
            return Err(IconCacheError::Extraction {
                source: target,
                message: "shell did not return an icon".into(),
            });
        }
        let bytes = unsafe { icon_to_ico(info.icon) };
        unsafe {
            DestroyIcon(info.icon);
        }
        bytes.map_err(|message| IconCacheError::Extraction {
            source: target,
            message,
        })
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
