# Rosetta Caching Research

- Status: open
- Priority: 1
- Tags:

## Description

## Goal

Add Rosetta AOT caching support for `linux/amd64` workloads in Bento VZ Linux guests on Apple silicon, without implementing it yet.

## Current Bento Status

- Rosetta support exists as a VM capability via `--rosetta`
- VM start fails early if host Rosetta is unavailable
- Rosetta is attached to the guest via virtio-fs tag `bento-rosetta`
- Guest bootstrap mounts `/mnt/bento-rosetta`
- Guest bootstrap registers Rosetta in `binfmt_misc`
- Kernel config enables `CONFIG_BINFMT_MISC=y`
- Docker verification path intentionally left for manual validation

## What Rosetta Caching Is

- Basic Rosetta translates `x86_64` Linux binaries on demand inside the guest
- Rosetta AOT caching stores translated artifacts so repeated runs are faster
- Apple exposes this through Virtualization.framework caching options
- Rosetta caching is separate from CDI
- CDI is only the container-runtime mechanism for injecting the Rosetta socket into containers

## Requirements

### Host

- Apple silicon
- VZ backend
- macOS 14 or newer for Rosetta AOT caching
- Rosetta installed on the host

### Guest

- Rosetta share mounted
- `binfmt_misc` configured
- `rosettad` running in the guest
- cache directory, socket permissions, and symlink configured correctly

## CDI Notes

CDI = Container Device Interface.

Purpose here:

- expose the Rosetta cache socket to containers in a standard way
- allow commands like:
  - `docker run --platform=linux/amd64 --device=<vendor>/rosetta=cached ...`

CDI does not perform translation itself.
It only injects mounts and related runtime configuration into containers.

Useful reference:

- https://github.com/cncf-tags/container-device-interface/blob/main/README.md

## How Lima Implements Rosetta Caching

### Host side

- creates a VZ Rosetta directory share
- if Rosetta is missing, Lima auto-installs it
- on macOS 14+, sets Rosetta caching options with:
  - `VZLinuxRosettaUnixSocketCachingOptions("/run/rosettad/rosetta.sock")`

Reference:

- `pkg/driver/vz/rosetta_directory_share_arm64.go`
- https://raw.githubusercontent.com/lima-vm/lima/master/pkg/driver/vz/rosetta_directory_share_arm64.go

### Guest side

- mounts Rosetta at `/mnt/lima-rosetta`
- registers `/mnt/lima-rosetta/rosetta` in `binfmt_misc`
- if `rosettad` exists, installs and starts it
- stores cache under `/var/cache/rosettad`
- uses real socket:
  - `/var/cache/rosettad/uds/rosetta.sock`
- symlinks expected socket path:
  - `/run/rosettad/rosetta.sock`
- fixes socket permissions so containers can use it

Reference:

- `pkg/driver/vz/boot.Linux/05-rosetta-volume.sh`
- https://raw.githubusercontent.com/lima-vm/lima/master/pkg/driver/vz/boot.Linux/05-rosetta-volume.sh

### CDI in Lima

- writes `/etc/cdi/rosetta.yaml`
- defines CDI kind:
  - `lima-vm.io/rosetta`
- defines device:
  - `cached`
- bind-mounts:
  - `/var/cache/rosettad/uds/rosetta.sock`
  - to `/run/rosettad/rosetta.sock` in the container
- adds BuildKit-friendly annotation:
  - `org.mobyproject.buildkit.device.autoallow: true`

User-facing examples:

- `docker run --platform=linux/amd64 --device=lima-vm.io/rosetta=cached ...`
- `RUN --device=lima-vm.io/rosetta=cached ...`

Reference:

- https://lima-vm.io/docs/config/multi-arch/#rosetta-aot-caching

## What SlicerVM Publicly Documents

Public docs only show:

- `rosetta: true` in config
- optional guest helper script to enable Rosetta in the guest

Publicly verified:

- Rosetta enablement exists
- Slicer uses Apple Virtualization.framework and VirtioFS

Not publicly verified:

- no public evidence of Rosetta AOT caching
- no public evidence of CDI
- no public evidence of `rosettad` socket wiring
- no public evidence of `VZLinuxRosettaUnixSocketCachingOptions`

References:

- https://docs.slicervm.com/mac/rosetta/
- https://docs.slicervm.com/mac/overview/

## Bento Implementation Plan

### Phase 1: minimal caching, no CDI

- add `rosetta_caching` VM capability
- require `--rosetta`
- validate `macOS >= 14`
- configure VZ Rosetta share with Unix socket caching option:
  - `/run/rosettad/rosetta.sock`
- add guest bootstrap/service for `rosettad`
- create cache dir:
  - `/var/cache/rosettad`
- create symlink:
  - `/run/rosettad/rosetta.sock` -> `/var/cache/rosettad/uds/rosetta.sock`

Outcome:

- direct guest execution can benefit from Rosetta caching
- lower implementation complexity

### Phase 2: full container integration with CDI

- generate `/etc/cdi/rosetta.yaml`
- choose Bento CDI kind, for example:
  - `bento-vm.io/rosetta`
- define device:
  - `cached`
- mount the Rosetta socket into containers
- document Docker/BuildKit usage

Outcome:

- Lima-style explicit container integration
- cleaner Docker and BuildKit support

## Recommended Bento Paths

### Host-side files

- `crates/bento-machine/src/backend/vz.rs`
- `crates/bento-machine/src/backend/vz/utils.rs`

### Config plumbing

- `crates/bentoctl/src/commands/create.rs`
- `crates/bento-runtime/src/instance.rs`
- `crates/bento-runtime/src/instance_store.rs`
- `crates/bento-machine/src/types.rs`
- `crates/bento-instanced/src/machine.rs`

### Guest bootstrap

- `crates/bento-instanced/src/bootstrap.rs`
- new task or service script under:
  - `crates/bento-instanced/scripts/tasks/`

## Suggested Design Choices

- do not auto-install Rosetta from Bento
- fail early with install instructions if Rosetta is missing
- make caching opt-in, separate from plain `--rosetta`
- implement caching without CDI first
- add CDI only when container-specific cached execution is worth the complexity

## Open Questions

- name of the Bento CDI device kind, probably:
  - `bento-vm.io/rosetta`
- whether to support only systemd guests for `rosettad` initially
- whether Docker CDI support should be documented as version-dependent
- whether containerd/nerdctl support matters or only Docker

## Manual Verification Ideas For Later

- confirm `rosettad` service is running
- confirm `/var/cache/rosettad` accumulates `.aotcache` files
- confirm `/run/rosettad/rosetta.sock` exists
- if CDI is added, confirm runtime discovers:
  - `bento-vm.io/rosetta=cached`
