//! Group icon asset integration built on the platform icon-cache adapter.
//!
//! The service is portable: extraction is injected, while Windows-only acquisition
//! remains behind `PlatformIconExtractor` in `platform::icon_cache`.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use crate::{
    domain::{Category, GroupIconSource},
    persistence::GroupPaths,
    platform::icon_cache::{cache_icon, CachePolicy, IconCacheError, IconExtractor, IconSource},
};

#[derive(Debug)]
pub enum AssetError {
    InvalidSource(String),
    Cache(IconCacheError),
    Io(io::Error),
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSource(value) => write!(f, "invalid group icon source: {value}"),
            Self::Cache(error) => write!(f, "group icon cache failed: {error}"),
            Self::Io(error) => write!(f, "group asset filesystem error: {error}"),
        }
    }
}
impl std::error::Error for AssetError {}
impl From<IconCacheError> for AssetError {
    fn from(error: IconCacheError) -> Self {
        Self::Cache(error)
    }
}
impl From<io::Error> for AssetError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAssets {
    pub icon: PathBuf,
    pub image: PathBuf,
    pub cached_icon: PathBuf,
}

#[derive(Debug, Clone)]
pub struct GroupAssetStore<E> {
    extractor: E,
    size: u32,
}

impl<E: IconExtractor> GroupAssetStore<E> {
    pub fn new(extractor: E, size: u32) -> Result<Self, AssetError> {
        if size == 0 {
            return Err(AssetError::InvalidSource(
                "asset size must be greater than zero".into(),
            ));
        }
        Ok(Self { extractor, size })
    }

    /// Reuse existing group outputs when neither the selected source nor shortcut
    /// targets changed. Callers provide the previous persisted model for that check.
    pub fn synchronize(
        &self,
        paths: &GroupPaths,
        category: &Category,
        previous: Option<&Category>,
    ) -> Result<Option<GroupAssets>, AssetError> {
        let Some(selection) = category.icon_source.as_ref() else {
            return Ok(None);
        };
        let changed = previous.map_or(true, |old| {
            old.icon_source != category.icon_source || old.shortcut_list != category.shortcut_list
        });
        if !changed && paths.group_icon.is_file() && paths.group_image.is_file() {
            return Ok(Some(GroupAssets {
                icon: paths.group_icon.clone(),
                image: paths.group_image.clone(),
                cached_icon: paths.group_icon.clone(),
            }));
        }
        let source = icon_source(selection)?;
        let policy = CachePolicy::new(&paths.icons, self.size)?;
        let cached = cache_icon(
            &policy,
            &self.extractor,
            paths.shortcut_file_name(),
            &source,
        )?;
        let bytes = fs::read(&cached.path)?;
        atomic_write(&paths.group_icon, &bytes)?;
        // The platform adapter emits ICO bytes. GroupImage is the legacy image
        // publication point; keeping the same authoritative bytes avoids a second
        // platform image codec and matches the adapter's lossless output.
        atomic_write(&paths.group_image, &bytes)?;
        Ok(Some(GroupAssets {
            icon: paths.group_icon.clone(),
            image: paths.group_image.clone(),
            cached_icon: cached.path,
        }))
    }
}

pub fn icon_source(selection: &GroupIconSource) -> Result<IconSource, AssetError> {
    if selection.path.trim().is_empty() {
        return Err(AssetError::InvalidSource("path is empty".into()));
    }
    let path = PathBuf::from(&selection.path);
    let lower = selection.path.to_ascii_lowercase();
    let base = if lower.ends_with(".lnk") {
        IconSource::Shortcut(path)
    } else if path.is_dir() {
        IconSource::Folder(path)
    } else {
        IconSource::Executable(path)
    };
    Ok(base.with_resource_index(selection.index))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "asset path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|x| x.to_str()).unwrap_or("asset")
    ));
    fs::write(&temporary, bytes)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temporary)?;
    file.sync_all()?;
    drop(file);
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ProgramShortcut;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fake;
    impl IconExtractor for Fake {
        fn extract(&self, _: &IconSource, size: u32) -> Result<Vec<u8>, IconCacheError> {
            Ok(format!("fake-{size}").into_bytes())
        }
    }
    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "taskbar-assets-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn creates_legacy_outputs_and_cache_with_fake_extractor() {
        let root = root();
        let app = crate::persistence::AppPaths::from_root(&root);
        let mut group = Category::new("Games");
        group.icon_source = Some(GroupIconSource {
            path: root.join("game.exe").to_string_lossy().into_owned(),
            index: 2,
        });
        group.shortcut_list.push(ProgramShortcut::new("game.exe"));
        let paths = app.group("Games").unwrap();
        fs::create_dir_all(&paths.directory).unwrap();
        fs::write(root.join("game.exe"), b"target").unwrap();
        let store = GroupAssetStore::new(Fake, 32).unwrap();
        let assets = store.synchronize(&paths, &group, None).unwrap().unwrap();
        assert_eq!(fs::read(assets.icon).unwrap(), b"fake-32");
        assert_eq!(fs::read(paths.group_image).unwrap(), b"fake-32");
        assert!(assets.cached_icon.is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unchanged_selection_preserves_existing_outputs() {
        let root = root();
        let app = crate::persistence::AppPaths::from_root(&root);
        let mut group = Category::new("Games");
        group.icon_source = Some(GroupIconSource {
            path: "game.exe".into(),
            index: 0,
        });
        let paths = app.group("Games").unwrap();
        fs::create_dir_all(&paths.directory).unwrap();
        fs::write(&paths.group_icon, b"old-icon").unwrap();
        fs::write(&paths.group_image, b"old-image").unwrap();
        let store = GroupAssetStore::new(Fake, 32).unwrap();
        store.synchronize(&paths, &group, Some(&group)).unwrap();
        assert_eq!(fs::read(paths.group_icon).unwrap(), b"old-icon");
        assert_eq!(fs::read(paths.group_image).unwrap(), b"old-image");
        let _ = fs::remove_dir_all(root);
    }
}
