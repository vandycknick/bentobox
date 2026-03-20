# 2. Image management

Date: 2026-02-20

## Status

Implemented

## Context

Bentobox needs an image workflow that supports:

- OCI-backed VM base images.
- A shared local image store that acts as the persistent cache.
- Pulling remote OCI images into that store.
- Importing OCI tar archives into that store for offline and cross-machine transfer.
- Packing a stopped local VM into the same store.
- Creating instances from image refs.
- Creating lower-level raw instances without requiring a base image.

The core design choice is that OCI is only the transport format. Bentobox persists normalized
images in its own local store and creates instances from that store. It does not keep a second
long-lived OCI blob cache in V1.

## Decision

### Local image store

Bentobox stores normalized images under `Directory::with_prefix("images").get_data_home()`:

- `$XDG_DATA_HOME/bento/images`, else
- `~/.local/share/bento/images`

Store layout:

```text
<images-root>/
  registry.json
  <image-id>/
    metadata.json
    rootfs.img
    kernel        # optional
    initramfs     # optional
```

`<image-id>` is derived from the OCI manifest digest by stripping the `sha256:` prefix.

`registry.json` stores image records and tag mappings. A record tracks:

- image ID and manifest digest
- source ref
- artifact type
- metadata payload
- rootfs path
- optional kernel/initramfs paths
- timestamps
- standard OCI annotations retained from the manifest

### OCI artifact format

Bentobox uses a standard OCI image manifest with custom payload layers.

- Artifact type: `application/vnd.bentobox.base-image.v1`
- Config media type: `application/vnd.oci.image.config.v1+json`
- Metadata layer: `application/vnd.bentobox.image.metadata.v1+json`
- Rootfs chunk layer: `application/vnd.bentobox.disk.chunk.v1+zstd`
- Kernel layer: `application/vnd.bentobox.boot.kernel.v1` (optional, at most one)
- Initramfs layer: `application/vnd.bentobox.boot.initramfs.v1` (optional, at most one)

Validation rules:

- exactly one metadata layer
- at least one rootfs chunk layer
- at most one kernel layer
- at most one initramfs layer

The rootfs payload is a raw disk split into fixed-size chunks and compressed chunk-by-chunk with
zstd. Chunks are reconstructed in manifest order into `rootfs.img` on ingest.

The metadata JSON is the source of truth for image defaults and capabilities:

```json
{
  "schemaVersion": 1,
  "os": "linux",
  "arch": "arm64",
  "defaults": {
    "cpu": 4,
    "memoryMiB": 4096
  },
  "bootstrap": {
    "cidataCloudInit": true
  },
  "extensions": {
    "ssh": true,
    "docker": false,
    "portForward": false
  }
}
```

Bundled boot assets are inferred from the presence of the optional kernel and initramfs layers, not
from metadata fields.

Bootstrap media for guest initialization is a separate concern from OCI image transport. Bentobox
uses a local NoCloud seed disk with volume label `CIDATA`, formatted as VFAT, so the same
bootstrap artifact can be attached on both VZ and Firecracker backends without host-specific ISO
tooling.

### Instance creation from images

`bentoctl create <ref> <name>` resolves a local image or pulls it on demand, then:

- applies image metadata defaults for CPU, memory, bootstrap capability, and extensions unless CLI
  overrides them
- prefers bundled `kernel` and `initramfs` when present unless CLI overrides them
- falls back to the global default kernel and initramfs bundle when the image does not provide them
- materializes the instance rootfs from the shared image store using `clonefile` on APFS when
  available, otherwise falls back to a normal copy

### Raw instance creation

`bentoctl create-raw <name>` is the lower-level workflow.

- default rootfs mode is no root disk
- `--rootfs <path>` attaches an existing root disk
- `--empty-rootfs <gb>` creates a sparse raw root disk in the instance directory

Backends must not assume a root disk always exists. Boot arguments should only include
`root=/dev/vda` when a root disk is actually attached.

### Packing local VMs

`bentoctl images pack <vm> <ref>`:

- requires a stopped VM with a root disk
- captures the VM rootfs and metadata into the Bentobox OCI format
- can optionally bundle the resolved kernel and/or initramfs with `--include-kernel` and
  `--include-initrd`
- ingests the resulting artifact into the shared local image store under `<ref>` by default

Optional pack output controls:

- `--outfile <path>` writes the generated OCI layout as a tar archive and skips importing it into
  the local image store
- `--debug` keeps the temporary OCI layout work directory on disk for inspection instead of deleting
  it after pack completes

Bundled boot asset rules:

- default: do not bundle kernel or initramfs
- `--include-kernel` resolves the VM-specific kernel first, then the global default
- `--include-initrd` resolves the VM-specific initramfs first, then the global default

### Pulling remote images

`bentoctl images pull <ref>` downloads the OCI artifact, validates it, reconstructs the normalized
image directory, and updates `registry.json`.

### Importing OCI tar archives

`bentoctl images import <path>` ingests an OCI tar archive into the normalized local image store.

- input is restricted to OCI tar archives in V1
- imported artifacts follow the same validation and reconstruction rules as pulled artifacts
- imported images converge onto the same local representation as pulled and packed images

Pulled and packed images must converge onto the same local representation.

### Backend disk policy

For file-backed Linux guests on the VZ backend, Bentobox sets explicit host caching and
synchronization defaults in the backend rather than relying on framework defaults.

Current policy:

- VZ Linux guests use cached disk image I/O with full synchronization
- the policy is private to the VZ backend
- when VZ macOS guests are added, the backend should choose the best guest-specific default there

Comparison of the backend features that matter here:

| Concern | VZ | Firecracker |
| --- | --- | --- |
| Host disk cache knob | Yes, `Automatic/Cached/Uncached` | No direct equivalent |
| Disk sync durability knob | Yes, `Full/Fsync/None` | No direct equivalent in the same API shape |
| Guest flush behavior control | Indirect via VZ sync mode | Yes, `cache_type = Unsafe/Writeback` |
| Good persistent default today | Linux: `Cached + Full` | Later: likely `Writeback` for persistent disks |
| Should this leak into shared machine types | No | No |

## Consequences

### Positive

- Pulled and packed images behave the same locally.
- Shared base images are stored once and cloned into instances from a single cache.
- The common image-backed workflow stays simple.
- Chunked compressed payloads reduce transfer overhead and keep retries smaller.
- Optional bundled boot assets improve portability without forcing every image to carry them.

### Negative

- The OCI artifact format is more complex than a single compressed disk blob.
- V1 does not keep a persistent OCI blob cache or implement CAS-style dedupe.
- `images push` is still deferred.
- Fallback copy can still lose sparse behavior when APFS clone is unavailable.

## Deferred

- `bentoctl images push <src> <ref>`
- registry credential integration
- signed artifact verification
- multi-arch index selection
- persistent OCI blob cache or CAS/dedupe layer if needed
- built-in `bentoctl images resize`
