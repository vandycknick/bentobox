use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use oci_client::client::{Client, ClientConfig};
use oci_client::manifest::OciManifest;
use oci_client::secrets::RegistryAuth;
use oci_client::Reference;
use oci_spec::image::{ImageIndex, ImageManifest, OciLayout};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::runtime::Builder;

use crate::directories::Directory;

const REGISTRY_INDEX_VERSION: u32 = 1;
const REGISTRY_FILE_NAME: &str = "registry.json";
const ARTIFACT_TYPE: &str = "application/vnd.bentobox.base-image.v1";
const CONFIG_MEDIA_TYPE: &str = "application/vnd.bentobox.base-image.config.v1+json";
const LAYER_MEDIA_TYPE_ZSTD: &str = "application/vnd.bentobox.disk.raw.v1+zstd";
const LAYER_MEDIA_TYPE_GZIP: &str = "application/vnd.bentobox.disk.raw.v1+gzip";
const OCI_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const OCI_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
const OCI_LAYOUT_VERSION: &str = "1.0.0";
const MISSING_ARTIFACT_TYPE: &str = "<missing> (possibly OCI container image manifest)";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageCompression {
    Zstd,
    Gzip,
}

impl ImageCompression {
    fn layer_media_type(self) -> &'static str {
        match self {
            Self::Zstd => LAYER_MEDIA_TYPE_ZSTD,
            Self::Gzip => LAYER_MEDIA_TYPE_GZIP,
        }
    }

    fn from_layer_media_type(media_type: &str) -> Result<Self, ImageStoreError> {
        match media_type {
            LAYER_MEDIA_TYPE_ZSTD => Ok(Self::Zstd),
            LAYER_MEDIA_TYPE_GZIP => Ok(Self::Gzip),
            _ => Err(ImageStoreError::UnsupportedMediaType {
                media_type: media_type.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRecord {
    pub id: String,
    pub source_ref: String,
    pub manifest_digest: String,
    pub artifact_type: String,
    pub compression: ImageCompression,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub rootfs_relpath: PathBuf,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageTag {
    pub name: String,
    pub image_id: String,
}

#[derive(Debug, Clone)]
pub struct TaggedImageRecord {
    pub tag: String,
    pub image: ImageRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryIndex {
    version: u32,
    #[serde(default)]
    tags: Vec<ImageTag>,
    images: Vec<ImageRecord>,
}

impl RegistryIndex {
    fn empty() -> Self {
        Self {
            version: REGISTRY_INDEX_VERSION,
            tags: Vec::new(),
            images: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct ImageStore {
    root: PathBuf,
    registry: RegistryIndex,
}

#[derive(Debug, Error)]
pub enum ImageStoreError {
    #[error("unable to resolve image store path from XDG_DATA_HOME or $HOME")]
    StoreRootUnavailable,

    #[error("failed to parse image reference {reference:?}: {source}")]
    InvalidReference {
        reference: String,
        #[source]
        source: oci_client::ParseError,
    },

    #[error("unsupported artifact type {artifact_type:?}, expected {expected:?}")]
    UnsupportedArtifactType {
        artifact_type: String,
        expected: &'static str,
    },

    #[error("unsupported media type {media_type:?}")]
    UnsupportedMediaType { media_type: String },

    #[error("manifest for {reference:?} has no layers")]
    MissingLayer { reference: String },

    #[error("failed to create OCI archive at {path}: {source}")]
    ArchiveCreate {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("import source is not a tar archive: {path} ({reason})")]
    ImportSourceNotTarArchive { path: PathBuf, reason: String },

    #[error("failed to extract tar archive: {path} ({reason})")]
    ImportTarExtractionFailed { path: PathBuf, reason: String },

    #[error("OCI layout is missing required file 'oci-layout' at {path}")]
    ImportMissingOciLayoutFile { path: PathBuf },

    #[error("failed to parse OCI layout file at {path}: {reason}")]
    ImportInvalidOciLayout { path: PathBuf, reason: String },

    #[error("unsupported OCI layout version {version:?} in {path}, expected {expected:?}")]
    ImportUnsupportedLayoutVersion {
        path: PathBuf,
        version: String,
        expected: &'static str,
    },

    #[error("OCI layout is missing required file 'index.json' at {path}")]
    ImportMissingIndexJson { path: PathBuf },

    #[error("failed to parse OCI index.json at {path}: {source}")]
    ImportInvalidIndexJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("OCI index at {path} has no manifest descriptors")]
    ImportMissingManifestDescriptor { path: PathBuf },

    #[error("OCI index at {path} has a manifest descriptor without digest")]
    ImportMissingManifestDigest { path: PathBuf },

    #[error("OCI manifest blob is missing for digest {digest} at {path}")]
    ImportMissingManifestBlob { path: PathBuf, digest: String },

    #[error("failed to parse OCI manifest at {path}: {source}")]
    ImportInvalidManifestJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("OCI manifest at {path} has no layer descriptors")]
    ImportMissingLayerDescriptor { path: PathBuf },

    #[error("OCI manifest layer at {path} is missing digest")]
    ImportMissingLayerDigest { path: PathBuf },

    #[error("OCI manifest layer at {path} is missing mediaType")]
    ImportMissingLayerMediaType { path: PathBuf },

    #[error("OCI layer blob is missing for digest {digest} at {path}")]
    ImportMissingLayerBlob { path: PathBuf, digest: String },

    #[error("failed to access image rootfs at {path}: {source}")]
    RootfsMetadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("registry at {path} is malformed")]
    InvalidRegistry { path: PathBuf },

    #[error("tag not found: {tag}")]
    TagNotFound { tag: String },

    #[error("OCI operation failed: {0}")]
    Oci(String),

    #[error("I/O failure")]
    Io(#[from] io::Error),

    #[error("JSON serialization/deserialization failure")]
    Json(#[from] serde_json::Error),
}

impl ImageStore {
    pub fn open() -> Result<Self, ImageStoreError> {
        let root = Directory::with_prefix("images")
            .get_data_home()
            .ok_or(ImageStoreError::StoreRootUnavailable)?;
        fs::create_dir_all(&root)?;

        let registry_path = root.join(REGISTRY_FILE_NAME);
        let registry = if registry_path.exists() {
            let data = fs::read(&registry_path)?;
            let reg: RegistryIndex = serde_json::from_slice(&data)?;
            if reg.version != REGISTRY_INDEX_VERSION {
                return Err(ImageStoreError::InvalidRegistry {
                    path: registry_path,
                });
            }
            reg
        } else {
            let reg = RegistryIndex::empty();
            write_atomic_json(&registry_path, &reg)?;
            reg
        };

        Ok(Self { root, registry })
    }

    pub fn list(&self) -> Result<Vec<TaggedImageRecord>, ImageStoreError> {
        let mut rows = Vec::new();
        for tag in &self.registry.tags {
            if let Some(image) = self
                .registry
                .images
                .iter()
                .find(|img| img.id == tag.image_id)
            {
                rows.push(TaggedImageRecord {
                    tag: tag.name.clone(),
                    image: image.clone(),
                });
            }
        }

        Ok(rows)
    }

    pub fn resolve(&self, name_or_ref: &str) -> Result<Option<ImageRecord>, ImageStoreError> {
        if let Some(tag) = self
            .registry
            .tags
            .iter()
            .find(|tag| tag.name == name_or_ref)
        {
            return Ok(self
                .registry
                .images
                .iter()
                .find(|img| img.id == tag.image_id)
                .cloned());
        }

        if let Some(record) = self
            .registry
            .images
            .iter()
            .find(|r| r.source_ref == name_or_ref)
            .cloned()
        {
            return Ok(Some(record));
        }

        Ok(None)
    }

    pub fn pull(
        &mut self,
        reference: &str,
        alias: Option<&str>,
    ) -> Result<ImageRecord, ImageStoreError> {
        let parsed = parse_reference(reference)?;
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(ImageStoreError::Io)?;
        let auth = RegistryAuth::Anonymous;
        let client = Client::new(ClientConfig::default());

        let (manifest_raw, manifest_digest) = runtime
            .block_on(client.pull_manifest_raw(&parsed, &auth, &[OCI_MANIFEST_MEDIA_TYPE]))
            .map_err(|err| ImageStoreError::Oci(err.to_string()))?;

        let manifest_value: serde_json::Value = serde_json::from_slice(manifest_raw.as_ref())?;
        let artifact_type = manifest_value
            .get("artifactType")
            .and_then(|v| v.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or(MISSING_ARTIFACT_TYPE)
            .to_string();

        if artifact_type != ARTIFACT_TYPE {
            return Err(ImageStoreError::UnsupportedArtifactType {
                artifact_type,
                expected: ARTIFACT_TYPE,
            });
        }

        let layer = manifest_value
            .get("layers")
            .and_then(|v| v.as_array())
            .and_then(|layers| layers.first())
            .ok_or_else(|| ImageStoreError::MissingLayer {
                reference: reference.to_string(),
            })?;

        let layer_media_type =
            layer
                .get("mediaType")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ImageStoreError::MissingLayer {
                    reference: reference.to_string(),
                })?;
        let compression = ImageCompression::from_layer_media_type(layer_media_type)?;

        let layer_digest = layer
            .get("digest")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ImageStoreError::MissingLayer {
                reference: reference.to_string(),
            })?
            .to_string();

        let image_id = image_id_from_digest(&manifest_digest);
        let image_dir = self.root.join(&image_id);
        fs::create_dir_all(&image_dir)?;

        let compressed_path = image_dir.join("rootfs.img.compressed");
        let out = runtime
            .block_on(tokio::fs::File::create(&compressed_path))
            .map_err(ImageStoreError::Io)?;

        runtime
            .block_on(client.pull_blob(&parsed, layer_digest.as_str(), out))
            .map_err(|err| ImageStoreError::Oci(err.to_string()))?;

        let rootfs_path = image_dir.join("rootfs.img");
        decompress_to_file(compression, &compressed_path, &rootfs_path)?;
        let _ = fs::remove_file(&compressed_path);

        let annotations = read_annotations(&manifest_value);
        let tag_name = alias
            .map(ToOwned::to_owned)
            .or_else(|| annotations.get("io.bentobox.image.name").cloned())
            .unwrap_or_else(|| default_name_from_reference(reference));

        let now = now_rfc3339();
        let mut record = ImageRecord {
            id: image_id.clone(),
            source_ref: reference.to_string(),
            manifest_digest,
            artifact_type: ARTIFACT_TYPE.to_string(),
            compression,
            os: annotations.get("io.bentobox.image.os").cloned(),
            arch: annotations.get("io.bentobox.image.arch").cloned(),
            rootfs_relpath: PathBuf::from(format!("{image_id}/rootfs.img")),
            created_at: now.clone(),
            updated_at: now,
            annotations,
        };

        if let Some(existing) = self.registry.images.iter().find(|r| r.id == record.id) {
            record.created_at = existing.created_at.clone();
        }

        self.upsert_record(record.clone())?;
        self.upsert_tag(tag_name, image_id)?;
        Ok(record)
    }

    pub fn import(&mut self, source: &Path) -> Result<ImageRecord, ImageStoreError> {
        let (layout_path, cleanup_path) = self.prepare_import_layout(source)?;

        let result = self.import_from_layout(source, &layout_path);

        if let Some(cleanup_path) = cleanup_path {
            let _ = fs::remove_dir_all(cleanup_path);
        }

        result
    }

    fn prepare_import_layout(
        &self,
        source: &Path,
    ) -> Result<(PathBuf, Option<PathBuf>), ImageStoreError> {
        if source.is_dir() {
            return Ok((source.to_path_buf(), None));
        }

        if !is_tar_file(source) {
            return Err(ImageStoreError::ImportSourceNotTarArchive {
                path: source.to_path_buf(),
                reason: "file is not a tar archive".to_string(),
            });
        }

        let temp = std::env::temp_dir().join(format!(
            "bento-image-import-{}-{}",
            std::process::id(),
            now_unix_nanos()
        ));
        fs::create_dir_all(&temp)?;

        let file = File::open(source)?;
        let mut archive = tar::Archive::new(file);
        archive
            .unpack(&temp)
            .map_err(|err| ImageStoreError::ImportTarExtractionFailed {
                path: source.to_path_buf(),
                reason: err.to_string(),
            })?;

        Ok((temp.clone(), Some(temp)))
    }

    fn import_from_layout(
        &mut self,
        source: &Path,
        layout_path: &Path,
    ) -> Result<ImageRecord, ImageStoreError> {
        let oci_layout_path = layout_path.join("oci-layout");
        if !oci_layout_path.is_file() {
            return Err(ImageStoreError::ImportMissingOciLayoutFile {
                path: oci_layout_path,
            });
        }

        let oci_layout = OciLayout::from_file(&oci_layout_path).map_err(|err| {
            ImageStoreError::ImportInvalidOciLayout {
                path: oci_layout_path.clone(),
                reason: err.to_string(),
            }
        })?;

        if oci_layout.image_layout_version() != OCI_LAYOUT_VERSION {
            return Err(ImageStoreError::ImportUnsupportedLayoutVersion {
                path: oci_layout_path,
                version: oci_layout.image_layout_version().clone(),
                expected: OCI_LAYOUT_VERSION,
            });
        }

        let index_path = layout_path.join("index.json");
        if !index_path.is_file() {
            return Err(ImageStoreError::ImportMissingIndexJson { path: index_path });
        }

        let index = ImageIndex::from_file(&index_path).map_err(|err| {
            ImageStoreError::ImportInvalidIndexJson {
                path: index_path.clone(),
                source: serde_json::Error::io(io::Error::other(err.to_string())),
            }
        })?;
        let descriptor = index.manifests().first().ok_or_else(|| {
            ImageStoreError::ImportMissingManifestDescriptor {
                path: index_path.clone(),
            }
        })?;

        let manifest_digest = descriptor.digest().to_string();
        let manifest_blob_path = blob_path(layout_path, &manifest_digest);
        if !manifest_blob_path.is_file() {
            return Err(ImageStoreError::ImportMissingManifestBlob {
                path: manifest_blob_path,
                digest: manifest_digest.clone(),
            });
        }

        let manifest = ImageManifest::from_file(&manifest_blob_path).map_err(|err| {
            ImageStoreError::ImportInvalidManifestJson {
                path: manifest_blob_path.clone(),
                source: serde_json::Error::io(io::Error::other(err.to_string())),
            }
        })?;

        let manifest_raw = fs::read(&manifest_blob_path)?;
        let manifest_value: serde_json::Value =
            serde_json::from_slice(&manifest_raw).map_err(|source| {
                ImageStoreError::ImportInvalidManifestJson {
                    path: manifest_blob_path.clone(),
                    source,
                }
            })?;

        let artifact_type = manifest
            .artifact_type()
            .as_ref()
            .map(ToString::to_string)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| MISSING_ARTIFACT_TYPE.to_string());
        if artifact_type != ARTIFACT_TYPE {
            return Err(ImageStoreError::UnsupportedArtifactType {
                artifact_type,
                expected: ARTIFACT_TYPE,
            });
        }

        let layer = manifest.layers().first().ok_or_else(|| {
            ImageStoreError::ImportMissingLayerDescriptor {
                path: manifest_blob_path.clone(),
            }
        })?;
        let layer_digest = layer.digest().to_string();
        if layer_digest.is_empty() {
            return Err(ImageStoreError::ImportMissingLayerDigest {
                path: manifest_blob_path.clone(),
            });
        }
        let media_type = layer.media_type().to_string();
        if media_type.is_empty() {
            return Err(ImageStoreError::ImportMissingLayerMediaType {
                path: manifest_blob_path.clone(),
            });
        }
        let compression = ImageCompression::from_layer_media_type(&media_type)?;

        let layer_blob_path = blob_path(layout_path, &layer_digest);
        if !layer_blob_path.is_file() {
            return Err(ImageStoreError::ImportMissingLayerBlob {
                path: layer_blob_path,
                digest: layer_digest.clone(),
            });
        }

        let image_id = image_id_from_digest(&manifest_digest);
        let image_dir = self.root.join(&image_id);
        fs::create_dir_all(&image_dir)?;
        let rootfs_path = image_dir.join("rootfs.img");
        decompress_to_file(
            compression,
            &blob_path(layout_path, &layer_digest),
            &rootfs_path,
        )?;

        let annotations = read_annotations(&manifest_value);
        let tag_name = annotations
            .get("io.bentobox.image.name")
            .cloned()
            .unwrap_or_else(|| image_id.clone());
        let now = now_rfc3339();
        let mut record = ImageRecord {
            id: image_id.clone(),
            source_ref: format!("oci-layout:{}", source.display()),
            manifest_digest,
            artifact_type: ARTIFACT_TYPE.to_string(),
            compression,
            os: annotations.get("io.bentobox.image.os").cloned(),
            arch: annotations.get("io.bentobox.image.arch").cloned(),
            rootfs_relpath: PathBuf::from(format!("{image_id}/rootfs.img")),
            created_at: now.clone(),
            updated_at: now,
            annotations,
        };

        if let Some(existing) = self.registry.images.iter().find(|r| r.id == record.id) {
            record.created_at = existing.created_at.clone();
        }

        self.upsert_record(record.clone())?;
        self.upsert_tag(tag_name, image_id)?;
        Ok(record)
    }

    pub fn pack_oci_archive(
        disk_image: &Path,
        name: &str,
        out_path: &Path,
        os: &str,
        arch: &str,
        compression: ImageCompression,
    ) -> Result<PathBuf, ImageStoreError> {
        let _ = parse_reference(name)?;

        let work_dir = std::env::temp_dir().join(format!("bento-pack-{}", now_unix_nanos()));
        fs::create_dir_all(&work_dir)?;
        let compressed = work_dir.join("layer.bin");
        let oci_layout_root = work_dir.join("layout");
        fs::create_dir_all(oci_layout_root.join("blobs/sha256"))?;

        compress_to_file(compression, disk_image, &compressed)?;
        let compressed_bytes = fs::read(&compressed)?;
        let layer_digest = format!("sha256:{}", sha256_hex(&compressed_bytes));
        let layer_size = compressed_bytes.len();
        write_blob(&oci_layout_root, &layer_digest, &compressed_bytes)?;

        let config_bytes = b"{}";
        let config_digest = format!("sha256:{}", sha256_hex(config_bytes));
        write_blob(&oci_layout_root, &config_digest, config_bytes)?;

        let mut annotations = BTreeMap::new();
        annotations.insert("io.bentobox.image.name".to_string(), name.to_string());
        annotations.insert("io.bentobox.image.os".to_string(), os.to_string());
        annotations.insert("io.bentobox.image.arch".to_string(), arch.to_string());
        annotations.insert(
            "org.opencontainers.image.created".to_string(),
            now_rfc3339(),
        );

        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": OCI_MANIFEST_MEDIA_TYPE,
            "artifactType": ARTIFACT_TYPE,
            "config": {
                "mediaType": CONFIG_MEDIA_TYPE,
                "digest": config_digest,
                "size": config_bytes.len(),
            },
            "layers": [
                {
                    "mediaType": compression.layer_media_type(),
                    "digest": layer_digest,
                    "size": layer_size,
                    "annotations": {
                        "org.opencontainers.image.title": "rootfs.img"
                    }
                }
            ],
            "annotations": annotations,
        });
        let _manifest_typed: oci_spec::image::ImageManifest =
            serde_json::from_value(manifest.clone())?;
        let manifest_bytes = serde_json::to_vec(&manifest)?;
        let manifest_digest = format!("sha256:{}", sha256_hex(&manifest_bytes));
        write_blob(&oci_layout_root, &manifest_digest, &manifest_bytes)?;

        let index = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": OCI_INDEX_MEDIA_TYPE,
            "manifests": [
                {
                    "mediaType": OCI_MANIFEST_MEDIA_TYPE,
                    "digest": manifest_digest,
                    "size": manifest_bytes.len(),
                    "annotations": {
                        "org.opencontainers.image.ref.name": name,
                    }
                }
            ]
        });
        fs::write(
            oci_layout_root.join("index.json"),
            serde_json::to_vec_pretty(&index)?,
        )?;

        let layout = serde_json::json!({
            "imageLayoutVersion": OCI_LAYOUT_VERSION,
        });
        let _layout_typed: oci_spec::image::OciLayout = serde_json::from_value(layout.clone())?;
        fs::write(
            oci_layout_root.join("oci-layout"),
            serde_json::to_vec_pretty(&layout)?,
        )?;

        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let out_file = File::create(out_path).map_err(|source| ImageStoreError::ArchiveCreate {
            path: out_path.to_path_buf(),
            source,
        })?;
        let mut tar_builder = tar::Builder::new(out_file);
        tar_builder.append_dir_all(".", &oci_layout_root)?;
        tar_builder.finish()?;

        let _ = fs::remove_dir_all(&work_dir);
        Ok(out_path.to_path_buf())
    }

    pub fn clone_base_image(
        &self,
        image: &ImageRecord,
        instance_rootfs: &Path,
    ) -> Result<(), ImageStoreError> {
        if let Some(parent) = instance_rootfs.parent() {
            fs::create_dir_all(parent)?;
        }

        let src = self.root.join(&image.rootfs_relpath);
        #[cfg(target_os = "macos")]
        {
            if try_clonefile(&src, instance_rootfs).is_ok() {
                return Ok(());
            }
        }

        fs::copy(src, instance_rootfs)?;
        Ok(())
    }

    pub fn remove_image(&mut self, tag_name: &str) -> Result<(), ImageStoreError> {
        let tag_idx = self
            .registry
            .tags
            .iter()
            .position(|tag| tag.name == tag_name)
            .ok_or_else(|| ImageStoreError::TagNotFound {
                tag: tag_name.to_string(),
            })?;

        let image_id = self.registry.tags[tag_idx].image_id.clone();
        self.registry.tags.remove(tag_idx);

        let still_referenced = self
            .registry
            .tags
            .iter()
            .any(|tag| tag.image_id == image_id);

        if !still_referenced {
            if let Some(image_idx) = self
                .registry
                .images
                .iter()
                .position(|img| img.id == image_id)
            {
                let image = self.registry.images.remove(image_idx);
                let image_dir = self.root.join(image.id);
                if image_dir.exists() {
                    fs::remove_dir_all(image_dir)?;
                }
            }
        }

        write_atomic_json(&self.root.join(REGISTRY_FILE_NAME), &self.registry)
            .map_err(ImageStoreError::Io)
    }

    pub fn image_rootfs_path(&self, image: &ImageRecord) -> PathBuf {
        self.root.join(&image.rootfs_relpath)
    }

    fn upsert_record(&mut self, record: ImageRecord) -> Result<(), ImageStoreError> {
        if let Some(existing) = self.registry.images.iter_mut().find(|r| r.id == record.id) {
            *existing = record;
        } else {
            self.registry.images.push(record);
        }

        write_atomic_json(&self.root.join(REGISTRY_FILE_NAME), &self.registry)
            .map_err(ImageStoreError::Io)
    }

    fn upsert_tag(&mut self, name: String, image_id: String) -> Result<(), ImageStoreError> {
        if let Some(existing) = self.registry.tags.iter_mut().find(|tag| tag.name == name) {
            existing.image_id = image_id;
        } else {
            self.registry.tags.push(ImageTag { name, image_id });
        }

        write_atomic_json(&self.root.join(REGISTRY_FILE_NAME), &self.registry)
            .map_err(ImageStoreError::Io)
    }
}

fn parse_reference(reference: &str) -> Result<Reference, ImageStoreError> {
    reference
        .parse::<Reference>()
        .map_err(|source| ImageStoreError::InvalidReference {
            reference: reference.to_string(),
            source,
        })
}

fn is_tar_file(path: &Path) -> bool {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    let mut archive = tar::Archive::new(file);
    let mut entries = match archive.entries() {
        Ok(e) => e,
        Err(_) => return false,
    };

    match entries.next() {
        None => false,            // no entries (or nothing readable)
        Some(Ok(_entry)) => true, // successfully parsed an entry header
        Some(Err(_)) => false,    // invalid tar (e.g., empty text file)
    }
}

fn decompress_to_file(
    compression: ImageCompression,
    source: &Path,
    destination: &Path,
) -> Result<(), ImageStoreError> {
    let input = File::open(source)?;
    let mut output = File::create(destination)?;
    let mut total_len: u64 = 0;
    let mut buf = [0u8; 1024 * 1024];

    match compression {
        ImageCompression::Zstd => {
            let mut decoder = zstd::Decoder::new(BufReader::new(input))?;
            loop {
                let n = decoder.read(&mut buf)?;
                if n == 0 {
                    break;
                }

                if buf[..n].iter().all(|byte| *byte == 0) {
                    output.seek(SeekFrom::Current(n as i64))?;
                } else {
                    output.write_all(&buf[..n])?;
                }

                total_len += n as u64;
            }
        }
        ImageCompression::Gzip => {
            let mut decoder = GzDecoder::new(BufReader::new(input));
            loop {
                let n = decoder.read(&mut buf)?;
                if n == 0 {
                    break;
                }

                if buf[..n].iter().all(|byte| *byte == 0) {
                    output.seek(SeekFrom::Current(n as i64))?;
                } else {
                    output.write_all(&buf[..n])?;
                }

                total_len += n as u64;
            }
        }
    }

    output.set_len(total_len)?;
    output.flush()?;
    Ok(())
}

fn compress_to_file(
    compression: ImageCompression,
    source: &Path,
    destination: &Path,
) -> Result<(), ImageStoreError> {
    let input = File::open(source)?;
    let mut reader = BufReader::new(input);
    let output = File::create(destination)?;

    match compression {
        ImageCompression::Zstd => {
            let mut encoder = zstd::Encoder::new(BufWriter::new(output), 8)?;
            io::copy(&mut reader, &mut encoder)?;
            encoder.finish()?.flush()?;
        }
        ImageCompression::Gzip => {
            let mut encoder = GzEncoder::new(BufWriter::new(output), Compression::default());
            io::copy(&mut reader, &mut encoder)?;
            encoder.finish()?.flush()?;
        }
    }

    Ok(())
}

fn write_atomic_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", now_unix_nanos()));
    let data = serde_json::to_vec_pretty(value).map_err(|err| io::Error::other(err.to_string()))?;
    fs::write(&tmp, data)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn write_blob(layout_root: &Path, digest: &str, data: &[u8]) -> Result<(), ImageStoreError> {
    let blob = blob_path(layout_root, digest);
    if let Some(parent) = blob.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(blob, data)?;
    Ok(())
}

fn now_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn image_id_from_digest(digest: &str) -> String {
    digest
        .strip_prefix("sha256:")
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| digest.to_string())
}

fn default_name_from_reference(reference: &str) -> String {
    reference
        .split('/')
        .next_back()
        .unwrap_or(reference)
        .to_string()
}

pub fn default_archive_name(image_name: &str) -> String {
    let safe = image_name
        .chars()
        .map(|ch| match ch {
            '/' | ':' | '@' => '-',
            _ => ch,
        })
        .collect::<String>();
    format!("{safe}.oci.tar")
}

fn blob_path(layout: &Path, digest: &str) -> PathBuf {
    let (alg, hash) = digest.split_once(':').unwrap_or(("sha256", digest));
    layout.join("blobs").join(alg).join(hash)
}

fn read_annotations(manifest: &serde_json::Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(map) = manifest.get("annotations").and_then(|v| v.as_object()) {
        for (k, v) in map {
            if let Some(value) = v.as_str() {
                out.insert(k.clone(), value.to_string());
            }
        }
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

#[cfg(target_os = "macos")]
fn try_clonefile(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid source path"))?;
    let dst = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid destination path"))?;

    let rc = unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut idx = 0usize;
    while size >= 1024.0 && idx < UNITS.len() - 1 {
        size /= 1024.0;
        idx += 1;
    }

    if idx == 0 {
        format!("{} {}", bytes, UNITS[idx])
    } else {
        format!("{size:.1} {}", UNITS[idx])
    }
}

pub fn artifact_type() -> &'static str {
    ARTIFACT_TYPE
}

pub fn image_size_bytes(store: &ImageStore, record: &ImageRecord) -> Result<u64, ImageStoreError> {
    let path = store.image_rootfs_path(record);
    let meta =
        fs::metadata(&path).map_err(|source| ImageStoreError::RootfsMetadata { path, source })?;
    Ok(meta.len())
}

pub fn is_supported_manifest(manifest: &OciManifest) -> bool {
    match manifest {
        OciManifest::Image(image) => image
            .artifact_type
            .as_deref()
            .map(|artifact| artifact == ARTIFACT_TYPE)
            .unwrap_or(false),
        OciManifest::ImageIndex(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bento-image-store-{name}-{}-{}",
            std::process::id(),
            now_unix_nanos()
        ))
    }

    #[test]
    fn human_size_formats_units() {
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn zstd_round_trip_compression() {
        let src = temp_path("zstd-src");
        let compressed = temp_path("zstd-packed");
        let out = temp_path("zstd-out");
        let data = b"hello image store".repeat(1024);
        fs::write(&src, &data).expect("source should be writable");

        compress_to_file(ImageCompression::Zstd, &src, &compressed).expect("compress should pass");
        decompress_to_file(ImageCompression::Zstd, &compressed, &out)
            .expect("decompress should pass");

        let got = fs::read(&out).expect("output should be readable");
        assert_eq!(got, data);

        let _ = fs::remove_file(src);
        let _ = fs::remove_file(compressed);
        let _ = fs::remove_file(out);
    }

    #[test]
    fn gzip_round_trip_compression() {
        let src = temp_path("gzip-src");
        let compressed = temp_path("gzip-packed");
        let out = temp_path("gzip-out");
        let data = b"hello image store".repeat(1024);
        fs::write(&src, &data).expect("source should be writable");

        compress_to_file(ImageCompression::Gzip, &src, &compressed).expect("compress should pass");
        decompress_to_file(ImageCompression::Gzip, &compressed, &out)
            .expect("decompress should pass");

        let got = fs::read(&out).expect("output should be readable");
        assert_eq!(got, data);

        let _ = fs::remove_file(src);
        let _ = fs::remove_file(compressed);
        let _ = fs::remove_file(out);
    }

    #[test]
    fn default_archive_name_is_oci_tar_and_sanitized() {
        let name = default_archive_name("ghcr.io/acme/base:1.0@sha256:abc");
        assert_eq!(name, "ghcr.io-acme-base-1.0-sha256-abc.oci.tar");
    }

    #[test]
    fn image_id_strips_sha256_prefix() {
        assert_eq!(
            image_id_from_digest("sha256:0123456789abcdef"),
            "0123456789abcdef"
        );
        assert_eq!(image_id_from_digest("abc"), "abc");
    }

    fn new_store(root: PathBuf) -> ImageStore {
        ImageStore {
            root,
            registry: RegistryIndex::empty(),
        }
    }

    #[test]
    fn import_non_tar_file_returns_specific_error() {
        let root = temp_path("import-nontar-root");
        fs::create_dir_all(&root).expect("root dir should be created");
        let mut store = new_store(root.clone());

        let source = temp_path("import-nontar-source");
        fs::write(&source, b"this is not a tar archive").expect("source file should be written");

        let err = store.import(&source).expect_err("import should fail");
        assert!(matches!(
            err,
            ImageStoreError::ImportSourceNotTarArchive { .. }
        ));

        let _ = fs::remove_file(source);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn import_corrupt_tar_returns_extraction_error() {
        let root = temp_path("import-corrupt-root");
        fs::create_dir_all(&root).expect("root dir should be created");
        let mut store = new_store(root.clone());

        let tar_path = temp_path("import-corrupt-source");
        {
            let file = File::create(&tar_path).expect("tar file should be created");
            let mut builder = tar::Builder::new(file);
            let data = b"hello";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "foo.txt", &data[..])
                .expect("tar should be written");
            builder.finish().expect("tar should finish");
        }

        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&tar_path)
            .expect("tar should be open for truncation");
        file.set_len(600)
            .expect("truncated tar should be creatable");

        let err = store.import(&tar_path).expect_err("import should fail");
        assert!(matches!(
            err,
            ImageStoreError::ImportTarExtractionFailed { .. }
        ));

        let _ = fs::remove_file(tar_path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn import_tar_without_oci_layout_returns_specific_error() {
        let root = temp_path("import-nonoci-root");
        fs::create_dir_all(&root).expect("root dir should be created");
        let mut store = new_store(root.clone());

        let tar_path = temp_path("import-nonoci-source");
        {
            let file = File::create(&tar_path).expect("tar file should be created");
            let mut builder = tar::Builder::new(file);
            let data = b"plain tar contents";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "foo.txt", &data[..])
                .expect("tar should be written");
            builder.finish().expect("tar should finish");
        }

        let err = store.import(&tar_path).expect_err("import should fail");
        assert!(matches!(
            err,
            ImageStoreError::ImportMissingOciLayoutFile { .. }
        ));

        let _ = fs::remove_file(tar_path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remove_image_removes_only_tag_when_aliases_exist() {
        let root = temp_path("remove-tag-only");
        fs::create_dir_all(&root).expect("root dir should be created");

        let image_id = "abc1234567890".to_string();
        let image_dir = root.join(&image_id);
        fs::create_dir_all(&image_dir).expect("image dir should be created");
        fs::write(image_dir.join("rootfs.img"), b"disk").expect("rootfs should exist");

        let mut store = ImageStore {
            root: root.clone(),
            registry: RegistryIndex {
                version: REGISTRY_INDEX_VERSION,
                tags: vec![
                    ImageTag {
                        name: "stable".to_string(),
                        image_id: image_id.clone(),
                    },
                    ImageTag {
                        name: "latest".to_string(),
                        image_id: image_id.clone(),
                    },
                ],
                images: vec![ImageRecord {
                    id: image_id.clone(),
                    source_ref: "example/ref:1".to_string(),
                    manifest_digest: "sha256:abc".to_string(),
                    artifact_type: ARTIFACT_TYPE.to_string(),
                    compression: ImageCompression::Zstd,
                    os: Some("linux".to_string()),
                    arch: Some("arm64".to_string()),
                    rootfs_relpath: PathBuf::from(format!("{image_id}/rootfs.img")),
                    created_at: now_rfc3339(),
                    updated_at: now_rfc3339(),
                    annotations: BTreeMap::new(),
                }],
            },
        };

        store
            .remove_image("stable")
            .expect("removing one tag should pass");

        assert_eq!(store.registry.tags.len(), 1);
        assert_eq!(store.registry.images.len(), 1);
        assert!(image_dir.exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remove_image_removes_volume_when_last_tag_deleted() {
        let root = temp_path("remove-last-tag");
        fs::create_dir_all(&root).expect("root dir should be created");

        let image_id = "fff1234567890".to_string();
        let image_dir = root.join(&image_id);
        fs::create_dir_all(&image_dir).expect("image dir should be created");
        fs::write(image_dir.join("rootfs.img"), b"disk").expect("rootfs should exist");

        let mut store = ImageStore {
            root: root.clone(),
            registry: RegistryIndex {
                version: REGISTRY_INDEX_VERSION,
                tags: vec![ImageTag {
                    name: "only".to_string(),
                    image_id: image_id.clone(),
                }],
                images: vec![ImageRecord {
                    id: image_id.clone(),
                    source_ref: "example/ref:1".to_string(),
                    manifest_digest: "sha256:fff".to_string(),
                    artifact_type: ARTIFACT_TYPE.to_string(),
                    compression: ImageCompression::Zstd,
                    os: Some("linux".to_string()),
                    arch: Some("arm64".to_string()),
                    rootfs_relpath: PathBuf::from(format!("{image_id}/rootfs.img")),
                    created_at: now_rfc3339(),
                    updated_at: now_rfc3339(),
                    annotations: BTreeMap::new(),
                }],
            },
        };

        store
            .remove_image("only")
            .expect("removing last tag should pass");

        assert!(store.registry.tags.is_empty());
        assert!(store.registry.images.is_empty());
        assert!(!image_dir.exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_uses_tags_and_list_returns_tag_rows() {
        let root = temp_path("resolve-tags");
        fs::create_dir_all(&root).expect("root dir should be created");

        let image_id = "1234567890abcdef".to_string();
        let store = ImageStore {
            root: root.clone(),
            registry: RegistryIndex {
                version: REGISTRY_INDEX_VERSION,
                tags: vec![
                    ImageTag {
                        name: "stable".to_string(),
                        image_id: image_id.clone(),
                    },
                    ImageTag {
                        name: "latest".to_string(),
                        image_id: image_id.clone(),
                    },
                ],
                images: vec![ImageRecord {
                    id: image_id.clone(),
                    source_ref: "example/ref:2".to_string(),
                    manifest_digest: "sha256:123".to_string(),
                    artifact_type: ARTIFACT_TYPE.to_string(),
                    compression: ImageCompression::Zstd,
                    os: Some("linux".to_string()),
                    arch: Some("arm64".to_string()),
                    rootfs_relpath: PathBuf::from(format!("{image_id}/rootfs.img")),
                    created_at: now_rfc3339(),
                    updated_at: now_rfc3339(),
                    annotations: BTreeMap::new(),
                }],
            },
        };

        let resolved = store
            .resolve("stable")
            .expect("resolve should pass")
            .expect("tag should resolve");
        assert_eq!(resolved.id, image_id);

        let rows = store.list().expect("list should pass");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row.tag == "stable"));
        assert!(rows.iter().any(|row| row.tag == "latest"));

        let _ = fs::remove_dir_all(root);
    }
}
