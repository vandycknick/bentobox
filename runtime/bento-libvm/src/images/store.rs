use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

const REGISTRY_VERSION: u32 = 1;
const REGISTRY_FILE_NAME: &str = "registry.json";
const ROOTFS_FILE_NAME: &str = "rootfs.img";

#[derive(Debug)]
pub struct ImageStore {
    root: PathBuf,
    registry: RegistryIndex,
}

#[derive(Debug, Clone)]
pub struct ResolvedImage {
    pub rootfs_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneBaseImageMethod {
    Clonefile,
    Copy,
}

#[derive(Debug, Deserialize)]
struct RegistryIndex {
    version: u32,
    #[serde(default)]
    images: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Error)]
pub enum ImageStoreError {
    #[error("image registry not found at {path}")]
    RegistryNotFound { path: PathBuf },

    #[error("image registry at {path} has version {version}, expected {expected}")]
    UnsupportedRegistryVersion {
        path: PathBuf,
        version: u32,
        expected: u32,
    },

    #[error("image reference {reference:?} is not present in {registry_path}")]
    ImageNotFound {
        reference: String,
        registry_path: PathBuf,
    },

    #[error("image reference {reference:?} maps to invalid rootfs path {path}: {reason}")]
    InvalidRootfsPath {
        reference: String,
        path: PathBuf,
        reason: &'static str,
    },

    #[error("image reference {reference:?} maps to missing rootfs at {path}")]
    RootfsNotFound { reference: String, path: PathBuf },

    #[error(
        "refusing to shrink raw disk {path} from {current_size} bytes to {requested_size} bytes"
    )]
    RawDiskShrinkUnsupported {
        path: PathBuf,
        current_size: u64,
        requested_size: u64,
    },

    #[error("I/O failure")]
    Io(#[from] io::Error),

    #[error("JSON serialization/deserialization failure")]
    Json(#[from] serde_json::Error),
}

impl ImageStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ImageStoreError> {
        let root = root.as_ref().to_path_buf();
        let registry_path = root.join(REGISTRY_FILE_NAME);
        let data = match fs::read(&registry_path) {
            Ok(data) => data,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(ImageStoreError::RegistryNotFound {
                    path: registry_path,
                });
            }
            Err(err) => return Err(ImageStoreError::Io(err)),
        };
        let registry: RegistryIndex = serde_json::from_slice(&data)?;

        if registry.version != REGISTRY_VERSION {
            return Err(ImageStoreError::UnsupportedRegistryVersion {
                path: registry_path,
                version: registry.version,
                expected: REGISTRY_VERSION,
            });
        }

        Ok(Self { root, registry })
    }

    pub fn resolve(&self, reference: &str) -> Result<ResolvedImage, ImageStoreError> {
        let registry_path = self.root.join(REGISTRY_FILE_NAME);
        let relative_rootfs =
            self.registry
                .images
                .get(reference)
                .ok_or_else(|| ImageStoreError::ImageNotFound {
                    reference: reference.to_string(),
                    registry_path: registry_path.clone(),
                })?;

        validate_rootfs_path(reference, relative_rootfs)?;

        let rootfs_path = self.root.join(relative_rootfs);
        if !rootfs_path.is_file() {
            return Err(ImageStoreError::RootfsNotFound {
                reference: reference.to_string(),
                path: rootfs_path,
            });
        }

        let canonical_root = fs::canonicalize(&self.root)?;
        let canonical_rootfs = fs::canonicalize(&rootfs_path)?;
        if !canonical_rootfs.starts_with(&canonical_root) {
            return Err(invalid_rootfs_path(
                reference,
                relative_rootfs,
                "path must stay within image store",
            ));
        }

        Ok(ResolvedImage { rootfs_path })
    }

    pub fn clone_base_image(
        &self,
        image: &ResolvedImage,
        instance_rootfs: &Path,
    ) -> Result<CloneBaseImageMethod, ImageStoreError> {
        if let Some(parent) = instance_rootfs.parent() {
            fs::create_dir_all(parent)?;
        }

        #[cfg(target_os = "macos")]
        {
            if try_clonefile(&image.rootfs_path, instance_rootfs).is_ok() {
                return Ok(CloneBaseImageMethod::Clonefile);
            }
        }

        fs::copy(&image.rootfs_path, instance_rootfs)?;
        Ok(CloneBaseImageMethod::Copy)
    }

    pub fn resize_raw_disk(path: &Path, size_bytes: u64) -> Result<(), ImageStoreError> {
        let file = File::options().write(true).open(path)?;
        let current_size = file.metadata()?.len();
        if size_bytes < current_size {
            return Err(ImageStoreError::RawDiskShrinkUnsupported {
                path: path.to_path_buf(),
                current_size,
                requested_size: size_bytes,
            });
        }

        file.set_len(size_bytes)?;
        Ok(())
    }
}

fn validate_rootfs_path(reference: &str, path: &Path) -> Result<(), ImageStoreError> {
    if path.as_os_str().is_empty() {
        return Err(invalid_rootfs_path(
            reference,
            path,
            "path must not be empty",
        ));
    }

    if path.is_absolute() {
        return Err(invalid_rootfs_path(
            reference,
            path,
            "path must be relative",
        ));
    }

    if path.file_name().and_then(|name| name.to_str()) != Some(ROOTFS_FILE_NAME) {
        return Err(invalid_rootfs_path(
            reference,
            path,
            "path must point to rootfs.img",
        ));
    }

    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::ParentDir => {
                return Err(invalid_rootfs_path(
                    reference,
                    path,
                    "path must not contain '..'",
                ));
            }
            Component::CurDir => {
                return Err(invalid_rootfs_path(
                    reference,
                    path,
                    "path must not contain '.'",
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(invalid_rootfs_path(
                    reference,
                    path,
                    "path must be relative",
                ));
            }
        }
    }

    Ok(())
}

fn invalid_rootfs_path(reference: &str, path: &Path, reason: &'static str) -> ImageStoreError {
    ImageStoreError::InvalidRootfsPath {
        reference: reference.to_string(),
        path: path.to_path_buf(),
        reason,
    }
}

#[cfg(target_os = "macos")]
fn try_clonefile(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid source path"))?;
    let dst = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid destination path"))?;

    // nix does not expose macOS clonefile(2), so call libc directly.
    // SAFETY: clonefile only reads these NUL-terminated paths during the call.
    let rc = unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::images::store::{CloneBaseImageMethod, ImageStore, ImageStoreError};

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bento-image-store-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ))
    }

    fn write_registry(root: &Path, body: &str) {
        fs::create_dir_all(root).expect("image root should be created");
        fs::write(root.join("registry.json"), body).expect("registry should be written");
    }

    #[test]
    fn resolve_uses_versioned_registry_mapping() {
        let root = temp_path("resolve");
        let image_dir = root.join("sha256-abc123");
        fs::create_dir_all(&image_dir).expect("image dir should be created");
        fs::write(image_dir.join("rootfs.img"), b"disk").expect("rootfs should be written");
        write_registry(
            &root,
            r#"{
                "version": 1,
                "images": {
                    "ghcr.io/vandycknick/archlinuxarm:latest": "sha256-abc123/rootfs.img"
                }
            }"#,
        );

        let store = ImageStore::open(&root).expect("store should open");
        let image = store
            .resolve("ghcr.io/vandycknick/archlinuxarm:latest")
            .expect("image should resolve");

        assert_eq!(image.rootfs_path, image_dir.join("rootfs.img"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_rejects_absolute_registry_path() {
        let root = temp_path("absolute");
        write_registry(
            &root,
            r#"{
                "version": 1,
                "images": {
                    "example/ref:latest": "/tmp/rootfs.img"
                }
            }"#,
        );

        let store = ImageStore::open(&root).expect("store should open");
        let err = store
            .resolve("example/ref:latest")
            .expect_err("absolute path should fail");

        assert!(matches!(err, ImageStoreError::InvalidRootfsPath { .. }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_rejects_parent_registry_path() {
        let root = temp_path("parent");
        write_registry(
            &root,
            r#"{
                "version": 1,
                "images": {
                    "example/ref:latest": "../rootfs.img"
                }
            }"#,
        );

        let store = ImageStore::open(&root).expect("store should open");
        let err = store
            .resolve("example/ref:latest")
            .expect_err("parent path should fail");

        assert!(matches!(err, ImageStoreError::InvalidRootfsPath { .. }));

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = temp_path("symlink-escape");
        let outside = temp_path("symlink-outside");
        fs::create_dir_all(&outside).expect("outside dir should be created");
        fs::write(outside.join("rootfs.img"), b"disk").expect("outside rootfs should be written");
        fs::create_dir_all(&root).expect("image root should be created");
        symlink(&outside, root.join("escape")).expect("symlink should be created");
        write_registry(
            &root,
            r#"{
                "version": 1,
                "images": {
                    "example/ref:latest": "escape/rootfs.img"
                }
            }"#,
        );

        let store = ImageStore::open(&root).expect("store should open");
        let err = store
            .resolve("example/ref:latest")
            .expect_err("symlink escape should fail");

        assert!(matches!(err, ImageStoreError::InvalidRootfsPath { .. }));

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn resolve_reports_missing_registry_entry() {
        let root = temp_path("missing-entry");
        write_registry(&root, r#"{"version": 1, "images": {}}"#);

        let store = ImageStore::open(&root).expect("store should open");
        let err = store
            .resolve("example/ref:latest")
            .expect_err("missing image should fail");

        assert!(matches!(err, ImageStoreError::ImageNotFound { .. }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_reports_missing_rootfs() {
        let root = temp_path("missing-rootfs");
        write_registry(
            &root,
            r#"{
                "version": 1,
                "images": {
                    "example/ref:latest": "sha256-missing/rootfs.img"
                }
            }"#,
        );

        let store = ImageStore::open(&root).expect("store should open");
        let err = store
            .resolve("example/ref:latest")
            .expect_err("missing rootfs should fail");

        assert!(matches!(err, ImageStoreError::RootfsNotFound { .. }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clone_base_image_reports_clone_method() {
        let root = temp_path("clone");
        let image_dir = root.join("sha256-clone");
        fs::create_dir_all(&image_dir).expect("image dir should be created");
        fs::write(image_dir.join("rootfs.img"), b"disk").expect("rootfs should be written");
        write_registry(
            &root,
            r#"{
                "version": 1,
                "images": {
                    "example/ref:clone": "sha256-clone/rootfs.img"
                }
            }"#,
        );

        let store = ImageStore::open(&root).expect("store should open");
        let image = store
            .resolve("example/ref:clone")
            .expect("image should resolve");
        let output = root.join("instance/rootfs.img");
        let method = store
            .clone_base_image(&image, &output)
            .expect("clone should succeed");

        #[cfg(target_os = "macos")]
        assert!(matches!(
            method,
            CloneBaseImageMethod::Clonefile | CloneBaseImageMethod::Copy
        ));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(method, CloneBaseImageMethod::Copy);
        assert_eq!(fs::read(output).expect("cloned disk should exist"), b"disk");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resize_raw_disk_grows_sparse_file() {
        let path = temp_path("resize-grow");
        let file = std::fs::File::create(&path).expect("raw disk should be creatable");
        file.set_len(512).expect("initial size should be set");

        ImageStore::resize_raw_disk(&path, 4096).expect("disk should grow");

        assert_eq!(
            fs::metadata(&path).expect("metadata should exist").len(),
            4096
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn resize_raw_disk_rejects_shrink() {
        let path = temp_path("resize-shrink");
        let file = std::fs::File::create(&path).expect("raw disk should be creatable");
        file.set_len(4096).expect("initial size should be set");

        let err = ImageStore::resize_raw_disk(&path, 512).expect_err("shrink should fail");
        assert!(matches!(
            err,
            ImageStoreError::RawDiskShrinkUnsupported { .. }
        ));

        let _ = fs::remove_file(path);
    }
}
