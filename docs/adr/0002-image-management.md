# 2. Image management

Date: 2026-02-20

## Status

Proposed

## Context

We need image management for VM base images that supports the following:

- OCI-backed base images for VM root disks.
- Local image store with unpacked images and metadata index.
- `bentoctl image` command group with `list`, `pull`, `import`, and `create`.
- `bentoctl create --image` that accepts local names or OCI references and auto-pulls when missing.
- Anonymous/public registry support in V1, credentials in V2.
- Compressed disk payloads (zstd default, gzip fallback).
- Existing runtime behavior where instance `rootfs.img` is used when present.

We also want to keep V1 simple and explicit:

- No CAS-first blob store.
- No built-in disk resize command yet.
- No private registry credential handling yet.
- No dedupe/GC/snapshot complexity yet.

The local directory location must reuse existing `Directory` resolution logic.

## Decision

We will implement a V1 image management system with a simple unpacked store and `registry.json` index.

### Directory and storage model

Use `Directory::with_prefix("images").get_data_home()` from `crates/bento-runtime/src/directories.rs`.

Image store root resolves to:

- `$XDG_DATA_HOME/bento/images`, else
- `~/.local/share/bento/images`

Store layout:

```text
<images-root>/
  registry.json
  <image-id>/
    rootfs.img
    source.json        # optional provenance snapshot
```

`<image-id>` is a stable local identifier derived from manifest digest.

### Registry index (`registry.json`)

Schema V1:

```json
{
  "version": 1,
  "images": [
    {
      "id": "sha256-<digest-no-colon>",
      "name": "ubuntu-24.04-arm64",
      "source_ref": "ghcr.io/example/ubuntu-base:24.04-arm64",
      "manifest_digest": "sha256:...",
      "artifact_type": "application/vnd.bentobox.base-image.v1",
      "compression": "zstd",
      "os": "ubuntu",
      "arch": "arm64",
      "rootfs_relpath": "sha256-.../rootfs.img",
      "created_at": "2026-02-20T12:34:56Z",
      "updated_at": "2026-02-20T12:34:56Z",
      "annotations": {
        "io.bentobox.image.name": "ubuntu-24.04-arm64"
      }
    }
  ]
}
```

Name resolution for `bentoctl create --image <value>`:

1. Match `images[].name == value`.
2. Else match `images[].source_ref == value`.
3. Else treat `value` as OCI reference and attempt pull.
4. If pull succeeds, resolve using newly inserted record.

### OCI artifact contract (V1)

- Artifact type: `application/vnd.bentobox.base-image.v1`
- Layer media types:
  - preferred: `application/vnd.bentobox.disk.raw.v1+zstd`
  - fallback: `application/vnd.bentobox.disk.raw.v1+gzip`
- Config media type: `application/vnd.bentobox.base-image.config.v1+json`
- Metadata annotations:
  - `io.bentobox.image.name` (fallback to `<repo>:<tag>` on ingest)
  - `io.bentobox.image.os`
  - `io.bentobox.image.arch`
  - `org.opencontainers.image.created`

Payload format is a compressed raw disk blob.

### Sparse and CoW behavior

- Pulled/imported base images are stored as `rootfs.img` in the image store.
- For `create --image`, materialize instance `rootfs.img` by:
  1. attempting APFS CoW clone via `clonefile`
  2. falling back to normal copy if clone fails

Notes:

- `clonefile` generally preserves CoW and sparse semantics.
- fallback copy may materialize holes and increase physical size, accepted in V1.

### CLI UX

#### `bentoctl image list`

`image list` prints a nicely formatted table with heading and all local records.

Required columns (in order):

1. `name`
2. `os`
3. `size`
4. `source_ref`
5. `arch`

`size` is the local `rootfs.img` file size in human-readable format.

Deferred column for V2:

- list/count of VMs using the base image.

#### `bentoctl image pull <oci-ref> [--name <alias>]`

- Pull anonymously from public OCI registries.
- Validate artifact type and supported compression media type.
- Decompress into `<images-root>/<image-id>/rootfs.img`.
- Insert/update `registry.json`.

#### `bentoctl image import <path>`

Supported inputs:

- OCI layout directory
- OCI archive tar

Behavior:

- parse manifest/index
- locate supported base-image artifact
- decode blob into local store
- update `registry.json`

#### `bentoctl image create --raw <path> --ref <oci-ref> [--name ...] [--os ...] [--arch ...]`

- read raw disk
- compress using zstd by default (gzip fallback supported)
- push blob + OCI artifact manifest to anonymous/public registry
- optionally register resulting image locally

#### `bentoctl create <instance-name> --image <name-or-ref> [existing flags...]`

- resolve or auto-pull base image
- materialize instance `rootfs.img`
- proceed with existing create flow
- runtime root disk probe picks up instance `rootfs.img`

### Runtime API design

Add `crates/bento-runtime/src/image_store.rs` and export it via `crates/bento-runtime/src/lib.rs`.

Key types:

```rust
pub enum ImageCompression { Zstd, Gzip }

pub struct ImageRecord {
    pub id: String,
    pub name: String,
    pub source_ref: String,
    pub manifest_digest: String,
    pub artifact_type: String,
    pub compression: ImageCompression,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub rootfs_relpath: std::path::PathBuf,
    pub created_at: String,
    pub updated_at: String,
    pub annotations: std::collections::BTreeMap<String, String>,
}

pub struct ImageStore { /* root path + registry state */ }
```

Key methods:

```rust
impl ImageStore {
    pub fn open() -> Result<Self, ImageStoreError>;
    pub fn list(&self) -> Result<Vec<ImageRecord>, ImageStoreError>;
    pub fn resolve(&self, name_or_ref: &str) -> Result<Option<ImageRecord>, ImageStoreError>;
    pub fn pull(&mut self, reference: &str, alias: Option<&str>) -> Result<ImageRecord, ImageStoreError>;
    pub fn import(&mut self, source: &std::path::Path) -> Result<ImageRecord, ImageStoreError>;
    pub fn create_artifact(&mut self, raw_disk: &std::path::Path, reference: &str, meta: CreateImageMeta) -> Result<ImageRecord, ImageStoreError>;
    pub fn materialize_instance_rootfs(&self, image: &ImageRecord, instance_rootfs: &std::path::Path) -> Result<(), ImageStoreError>;
}
```

### Dependency plan

Add to `crates/bento-runtime/Cargo.toml`:

- `oci-client`
- `oci-spec`
- `tokio`
- `zstd`
- `flate2`
- `tar`
- `sha2` (if needed)
- `tempfile` (if needed)

No shell-out to external CLI tools for OCI/compression flows.

### Error model

`ImageStoreError` will cover:

- store path resolution failures
- registry JSON load/save/parse errors
- OCI reference/registry errors
- unsupported artifact/compression/media type
- compression encode/decode errors
- file IO + atomic write failures
- clone/copy materialization failures

All user-facing errors must include actionable context (reference/path/media type).

### Manual disk expansion (V1)

No built-in resize command in V1. Manual external CLI usage is supported.

Host-side grow file example:

```bash
truncate -s +10G /path/to/instance/rootfs.img
```

Common guest-side filesystem grow examples:

```bash
sudo growpart /dev/vda 1
sudo resize2fs /dev/vda1
```

or

```bash
sudo growpart /dev/vda 1
sudo xfs_growfs /
```

Verify in guest:

```bash
lsblk
df -h
```

## Consequences

### Positive

- Clear, simple V1 image workflow with low implementation risk.
- Consistent local storage path via existing `Directory` abstraction.
- Easy user flow for instance creation from local names or OCI references.
- Compressed payloads reduce practical transfer/storage overhead for sparse-ish raw disks.
- Keeps runtime boot logic mostly unchanged.

### Negative

- No private registry credentials in V1.
- Fallback copy may lose sparse behavior and increase disk usage.
- No built-in resize command yet.
- No dedupe/GC/CAS optimizations.

### Deferred (V2+)

1. Registry credential integration.
2. Signed artifact verification.
3. Multi-arch index selection.
4. GC/pruning and dedupe.
5. Built-in `bentoctl image resize`.
6. `image list` column showing VM usage per base image.

## Implementation Plan

### Phase 1: Foundation and list

- [ ] Create `image_store.rs` module and export in `lib.rs`.
- [ ] Implement store root resolution via `Directory::with_prefix("images")`.
- [ ] Implement `registry.json` load/create/save with atomic writes.
- [ ] Implement `ImageStore::list`.
- [ ] Add `bentoctl image` command wiring in `commands/mod.rs`.
- [ ] Implement `bentoctl image list`.
- [ ] Implement table renderer with heading and columns: `name`, `os`, `size`, `source_ref`, `arch`.

### Phase 2: Pull and install

- [ ] Implement anonymous pull pipeline with `oci-client`.
- [ ] Validate artifact type and layer media type.
- [ ] Implement zstd decode path.
- [ ] Implement gzip decode fallback path.
- [ ] Write decoded `rootfs.img` atomically into image dir.
- [ ] Extract annotations and fallback naming logic.
- [ ] Upsert image record in `registry.json`.
- [ ] Add `bentoctl image pull` command.

### Phase 3: Create integration

- [ ] Extend `bentoctl create` with `--image`.
- [ ] Resolve local image by name/source ref.
- [ ] Auto-pull on cache miss.
- [ ] Materialize instance `rootfs.img` from base image.
- [ ] Try `clonefile` first.
- [ ] Fallback to normal copy if clone fails.
- [ ] Keep existing runtime boot flow unchanged.

### Phase 4: Publish (`image create`)

- [ ] Implement raw disk to compressed blob (zstd default).
- [ ] Support gzip path as fallback.
- [ ] Push blob + manifest using `oci-client` + `oci-spec`.
- [ ] Apply metadata annotations (`name`, `os`, `arch`).
- [ ] Add `bentoctl image create` command.
- [ ] Register local image metadata after successful push (or pull-back).

### Phase 5: Import

- [ ] Implement `bentoctl image import` for OCI layout directory.
- [ ] Implement `bentoctl image import` for OCI archive tar.
- [ ] Reuse pull ingest pipeline for decode + registration.

### Phase 6: Docs and validation

- [ ] Update README/docs with image management usage.
- [ ] Document manual disk expansion workflow.
- [ ] Add unit tests for store/index/compression paths.
- [ ] Add integration tests for list/pull/create/import flows.
- [ ] Run `cargo fmt`.
- [ ] Run `cargo clippy --all --benches --tests --examples --all-features`.
- [ ] Run targeted tests for touched runtime/CLI paths.
