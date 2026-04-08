# 4. Daemonless architecture

Date: 2026-04-06

## Status

Proposed

## Context

Bentobox currently mixes responsibilities across the CLI, runtime helpers, and `instanced`.

Today:

- `bentoctl` still owns business logic that should live in a library.
- `bentoctl` directly starts `instanced` and polls pidfiles and sockets to infer startup.
- `bento-runtime` mixes domain types with machine storage, image store, profile resolution, and transport helpers.
- `bento-instanced` mixes VM preparation, runtime supervision, bootstrap logic, and control APIs.

This makes the architecture harder to evolve toward a Podman-style split where:

- the CLI is a thin consumer of a library,
- the engine can operate in local ABI mode or future tunnel mode,
- one monitor process supervises one VM,
- and the monitor can be swapped from direct local control to a future remote manager API without changing CLI behavior.

We want to preserve the rough no-central-daemon model used by Podman/libpod/conmon, while leaving room for a future remote manager API. The immediate goal is local ABI mode, not a permanent central daemon.

## Goals

- Make `bentoctl` a thin frontend over a library API.
- Introduce a clear engine crate, `bento-libvm`, that owns machine lifecycle and storage.
- Introduce a clear shared domain crate, `bento-core`, that owns the canonical machine `VmSpec` and related models.
- Rename and refactor `bento-instanced` into `bento-vmmon`.
- Make `bento-vmmon` one process per running VM.
- Move monitor spawn and startup ownership from the CLI into `bento-libvm`.
- Keep `config.yaml` in the per-instance directory as the canonical machine configuration.
- Use SQLite (WAL mode) for manager metadata and indexes, including UUID-to-name mapping.
- Preserve the existing Negotiate protocol model so `vmmon` can still upgrade a connection to serial attach, vsock access, or RPC.
- Define an API split that supports local ABI mode now and future tunnel mode later.

## Non-goals

- Implement the future remote manager daemon in this phase.
- Introduce a long-lived always-on daemon as the primary architecture.
- Redesign the current Negotiate protocol.
- Finalize the full SQLite schema beyond the responsibilities and canonical data ownership defined here.

## Decision

We will adopt the following architecture:

- `bento-runtime` will be collapsed into `bento-libvm` except for shared domain models moved into `bento-core`.
- `bento-core` will define the canonical machine `VmSpec` and shared domain types.
- `bento-libvm` will own machine lifecycle, on-disk layout, inventory, image and profile policy, bootstrap materialization, Negotiate client behavior, and `vmmon` process spawning.
- `bento-vmmon` will be a refactor and rename of `bento-instanced` into a per-VM runtime supervisor.
- the monitor package will be `bento-vmmon` and the executable will be `vmmon`.
- `bento-vmmon` will read the machine `config.yaml` from the instance directory and auto-start the VM on spawn.
- `bento-vmmon` will daemonize itself by default and support a foreground mode for tests and debugging.
- `bento-libvm` and `bento-vmmon` will synchronize startup using a dedicated startup pipe, not an RPC `Start` call.
- `bento-libvm` will stop `bento-vmmon` using signals. `Stop` will not be part of `VmMonitorService`.
- during migration, `bento-libvm` may temporarily use the monitor pidfile to signal `vmmon` for stop and to observe shutdown completion.
- the daemonizing parent `vmmon` process forwards the child monitor startup result over the original startup pipe so `bento-libvm` still sees a single startup handshake.
- The machine stable identity will be a UUID stored in SQLite and used as the per-instance directory name.
- The canonical per-instance data directory will be `~/.local/share/bento/instances/<uuid>/`.
- The canonical SQLite database location will be `~/.local/share/bento/state.db`.
- `InstanceService` will be the manager API.
- `VmMonitorService` will be the per-VM monitor API.
- The existing Negotiate protocol will remain, implemented server-side in `bento-vmmon` and client-side in `bento-libvm`.

## Component Boundaries

### `bentoctl`

`bentoctl` is a thin command-line frontend.

It owns:

- argument parsing,
- output formatting,
- user interaction,
- calling `bento-libvm`,
- stdio proxying and local terminal handling once a monitor stream has already been opened by `bento-libvm`.

It does not own:

- machine business logic,
- machine creation policy,
- monitor spawning,
- pidfile polling,
- direct Negotiate client usage,
- direct `vmmon` lifecycle management.

### `bento-core`

`bento-core` owns shared domain models.

It owns:

- the canonical machine `VmSpec`,
- backend, architecture, platform, network, and storage enums and structs,
- machine metadata types shared across crates,
- restart policy and label models,
- UUID-backed machine identity types.

It does not own:

- SQLite,
- filesystem layout,
- image store policy,
- process management,
- RPC servers or clients,
- backend-specific VM execution logic.

### `bento-libvm`

`bento-libvm` is the machine engine library.

It owns:

- machine creation, start, stop, remove, list, and inspect,
- the top-level data directory and instance directory layout,
- SQLite state at `~/.local/share/bento/state.db`,
- UUID allocation,
- mapping UUID to name and name to UUID,
- instance metadata, labels, creation timestamps, and restart policy,
- writing the canonical `config.yaml` from `bento-core::VmSpec`,
- image resolution and instance materialization,
- profile resolution,
- bootstrap and host-side integration policy,
- spawning `bento-vmmon` in local ABI mode,
- client-side Negotiate logic,
- monitor connection logic for status, service readiness, and negotiated proxy streams,
- client-side manager API abstraction for local ABI and future tunnel mode.

It does not own:

- direct backend-specific VM lifecycle implementation,
- long-running per-VM supervision,
- guest-agent implementation.

### `bento-vmmon`

`bento-vmmon` is the per-VM monitor and supervisor.

It owns:

- reading `config.yaml` from the instance directory,
- daemonizing itself,
- writing its pidfile and runtime artifacts,
- writing durable exit metadata in `exit.json`,
- starting and supervising one VM,
- exposing `VmMonitorService`,
- implementing the Negotiate server for serial attach, vsock connect, and monitor RPC upgrades,
- tracking runtime state for one running VM,
- runtime cleanup and exit state persistence,
- signal-driven shutdown.

During migration, `bento-vmmon` may temporarily keep legacy paths for older name-based startup flows, but the data-dir-driven monitor path now consumes `bento-core::VmSpec` directly instead of adapting it through legacy runtime config structures.

It does not own:

- the global machine inventory,
- machine name lookup across all machines,
- image resolution,
- machine creation policy,
- manager-level start, stop, list, or remove APIs,
- how the machine was originally created.

### `bento-vmm`

`bento-vmm` remains the backend abstraction.

It owns:

- backend-specific VM execution,
- backend validation,
- backend process lifecycle primitives,
- serial and vsock hooks exposed to the monitor.

It does not own:

- machine identity,
- machine inventory,
- manager APIs,
- monitor daemonization,
- on-disk manager state,
- profile or image policy.

## State Model

### Canonical machine identity

- Every machine has a stable UUID.
- The UUID is the manager identity used in SQLite.
- The per-instance directory name is that UUID (32 lowercase hex chars, no dashes).
- Human-readable names are aliases resolved through SQLite.

### Canonical machine config

- `~/.local/share/bento/instances/<ulid>/config.yaml` is the canonical machine configuration.
- `config.yaml` is written by `bento-libvm` from `bento-core::VmSpec`.
- `bento-vmmon` only needs the instance directory path and reads `config.yaml` from there.

### Manager metadata and indexes

`~/.local/share/bento/state.db` (SQLite, WAL mode) stores:

- UUID to name mapping,
- name to UUID mapping,
- creation time,
- instance directory path.

SQLite does not replace `config.yaml` as the canonical boot input.

### Observed runtime state

Runtime truth comes from `bento-vmmon` when it is running.

Manager metadata may cache last-known runtime state, but runtime liveness and readiness are observed from the running monitor and not inferred solely from SQLite.

## On-disk Layout

The default layout is:

```text
~/.local/share/bento/
  state.db
  instances/
    <uuid>/
      config.yaml
      vmmon.pid
      vmmon.sock
      exit.json
      logs/
      runtime/
  images/
    ...
```

`images/` remains manager-owned data.

## API Model

### `InstanceService`

`InstanceService` is the manager API shape exposed by `bento-libvm` locally and by a future remote manager service in tunnel mode.

It includes:

- `Create`
- `Start`
- `Stop`
- `Remove`
- `List`
- `Inspect`
- `SshInfo`

### `VmMonitorService`

`VmMonitorService` is the per-VM monitor API exposed by `bento-vmmon`.

It includes:

- `Ping`
- `Inspect`
- `WatchStatus`

It does not include `Stop`.

Shutdown is signal-based and owned by `bento-libvm`.

## Negotiate Protocol Ownership

The current Negotiate protocol remains part of the architecture.

It is used to upgrade a connection into:

- a serial attach session,
- a vsock connection into the guest,
- an RPC channel.

Ownership becomes:

- `bento-vmmon`: Negotiate server implementation,
- `bento-libvm`: Negotiate client implementation.

This preserves the current upgrade model while moving ownership out of the CLI.

## `bento-vmmon` Process Model

`bento-vmmon` should follow a structure similar to `arcbox-daemon` at the top level:

- `main.rs` handles argument parsing, logging setup, Tokio runtime creation, and resolving the instance data directory.
- `run()` coordinates staged startup.
- helper modules own startup, services, shutdown, runtime state, and shared context.

Recommended high-level module split:

- `main.rs`
- `run.rs`
- `startup.rs`
- `services.rs`
- `shutdown.rs`
- `state.rs`
- `context.rs`
- `supervisor.rs`

The `vmmon` executable accepts a `--data-dir` argument that points at one specific instance directory, for example:

```text
~/.local/share/bento/instances/<ulid>
```

`vmmon` is data-dir-driven. It accepts `--data-dir` and reads `config.yaml` from that directory as its startup contract.

## Startup and Shutdown Semantics

### Startup synchronization

`bento-libvm` spawns `bento-vmmon` and passes a startup pipe.

`bento-vmmon` uses the startup pipe to report either:

- successful supervision startup, or
- structured startup failure.

We explicitly do not use an RPC `Start` call for the initial handshake because the monitor socket does not exist until after monitor initialization. Using RPC for startup would reintroduce socket polling and readiness races.

### Startup success threshold

`Start` succeeds when:

- `bento-vmmon` has daemonized or entered foreground mode successfully,
- `config.yaml` has been loaded,
- runtime artifacts such as pidfile and socket are initialized,
- the backend has been created,
- VM start has been invoked successfully,
- the supervisor loop is live.

`Start` does not require guest readiness, SSH reachability, or guest service readiness.

### Startup pipe result

The startup pipe reports a one-shot result similar to:

```text
Started {
  vmmon_pid,
  socket_path,
  vm_pid?,
}

Failed {
  stage,
  message,
}
```

Typical `stage` values:

- `load_config`
- `daemonize`
- `bind_socket`
- `create_backend`
- `start_vm`

### Shutdown

`bento-libvm` stops a running machine by signaling `bento-vmmon`.

Recommended behavior:

- `SIGTERM` requests graceful shutdown.
- `SIGINT` may also request graceful shutdown for local interactive compatibility.
- A second signal or shutdown timeout escalates to forced teardown.
- `bento-vmmon` persists exit metadata before exiting.

Stop is therefore manager-owned and signal-driven, not monitor-RPC-owned.

## Proposed `bento-core::VmSpec`

The top-level machine config type is named `VmSpec`.

`VmSpec` is the only type in `bento-core` that uses the `Spec` suffix.

Supporting types do not use a `Vm` prefix or `Spec` suffix.

Initial proposed shape:

```rust
pub struct VmSpec {
    pub version: u32,
    pub name: String,
    pub platform: Platform,
    pub resources: Resources,
    pub boot: Boot,
    pub storage: Storage,
    pub mounts: Vec<Mount>,
    pub network: Network,
    pub guest: Guest,
    pub host: Host,
}

pub struct Platform {
    pub guest_os: GuestOs,
    pub architecture: Architecture,
    pub backend: Backend,
}

pub struct Resources {
    pub cpus: u8,
    pub memory_mib: u32,
}

pub struct Boot {
    pub kernel: Option<std::path::PathBuf>,
    pub initramfs: Option<std::path::PathBuf>,
    pub kernel_cmdline: Vec<String>,
    pub bootstrap: Option<Bootstrap>,
}

pub struct Bootstrap {
    pub cloud_init: Option<std::path::PathBuf>,
}

pub struct Storage {
    pub disks: Vec<Disk>,
}

pub struct Disk {
    pub path: std::path::PathBuf,
    pub kind: DiskKind,
    pub read_only: bool,
}

pub enum DiskKind {
    Root,
    Data,
    Seed,
}

pub struct Mount {
    pub source: std::path::PathBuf,
    pub tag: String,
    pub read_only: bool,
}

pub struct Network {
    pub mode: NetworkMode,
}

pub struct Guest {
    pub profiles: Vec<String>,
    pub capabilities: Capabilities,
}

pub struct Host {
    pub nested_virtualization: bool,
    pub rosetta: bool,
}

pub enum GuestOs {
    Linux,
}

pub enum Architecture {
    Aarch64,
    X86_64,
}

pub enum Backend {
    Auto,
    Vz,
    Firecracker,
    CloudHypervisor,
}

pub enum NetworkMode {
    None,
    User,
    Bridged,
}

pub struct Capabilities {
    pub ssh: bool,
    pub docker: bool,
    pub dns: bool,
    pub forward: bool,
}
```

This shape is intentionally manager-facing and canonical. Backend-private derived configuration remains outside `VmSpec`.

## Sequence Diagrams

### Local ABI start flow

```text
CLI/SDK -> libvm: Start(name or uuid)
libvm -> sqlite: resolve name -> uuid
libvm -> vmmon: spawn --data-dir ~/.local/share/bento/instances/<uuid> --startup-fd N
vmmon -> vmmon: parse args, init logging, build tokio runtime
vmmon -> vmmon: daemonize
vmmon -> config.yaml: load VmSpec
vmmon -> vmmon: bind monitor socket, write pidfile
vmmon -> vmm: create VM
vmmon -> vmm: start VM
vmmon -> libvm: Started { vmmon_pid, socket_path, vm_pid? }
libvm -> CLI/SDK: success
```

### Graceful stop flow

```text
CLI/SDK -> libvm: Stop(name or uuid)
libvm -> sqlite: resolve name -> uuid
libvm -> vmmon: SIGTERM
vmmon -> vmmon: transition to stopping
vmmon -> vmm: graceful stop
vmm -> vmmon: VM exits
vmmon -> exit.json: persist exit metadata
vmmon -> libvm-visible state: stopped
vmmon -> vmmon: remove runtime artifacts and exit
libvm -> CLI/SDK: stop requested
```

### Forced stop escalation

```text
CLI/SDK -> libvm: Stop(name or uuid)
libvm -> vmmon: SIGTERM
vmmon -> vmm: graceful stop
... timeout expires ...
libvm or vmmon -> vmmon/backend: escalate
vmmon -> vmm/helper processes: force teardown
vmmon -> exit.json: persist forced exit metadata
vmmon -> vmmon: exit
```

## Implementation Phases

### Phase 1: architecture and contracts

- write and approve this ADR,
- define crate responsibilities,
- define the canonical `VmSpec`,
- define on-disk layout,
- define startup pipe semantics,
- define `InstanceService` and `VmMonitorService` responsibilities.

### Phase 2: introduce `bento-core`

- add `bento-core`,
- move shared domain types into it,
- define UUID-backed machine identity types,
- add serialization for `VmSpec`.

### Phase 3: introduce `bento-libvm`

- add `bento-libvm`,
- move runtime policy and manager logic out of `bento-runtime`,
- move CLI-owned lifecycle logic into `bento-libvm`,
- keep temporary adapters as needed.

### Phase 4: adopt manager state and layout

- add SQLite at `~/.local/share/bento/state.db`,
- allocate UUIDs on create,
- move instance directories to `~/.local/share/bento/instances/<uuid>/`,
- write canonical `config.yaml`,
- store name mappings and metadata in SQLite.

### Phase 5: rename and restructure monitor

- rename `bento-instanced` to `bento-vmmon`,
- reorganize to a thin `main` plus staged `run()`,
- add `--data-dir`,
- move daemonization into `vmmon`,
- add startup pipe handshake,
- add signal-driven shutdown.

### Phase 6: split APIs and Negotiate ownership

- define manager `InstanceService`,
- define per-VM `VmMonitorService`,
- move Negotiate server ownership into `bento-vmmon`,
- move Negotiate client ownership into `bento-libvm`,
- route CLI commands through `bento-libvm`.

### Phase 7: remove obsolete seams

- remove direct CLI monitor spawning,
- remove CLI-owned pidfile polling startup logic,
- delete or shrink obsolete `bento-runtime` modules,
- update tests and docs to the new architecture.

## Consequences

### Positive

- CLI behavior becomes independent from local versus future remote manager mode.
- Machine lifecycle logic becomes reusable from SDKs and other language bindings.
- One monitor per running VM creates a clear operational boundary.
- Startup error propagation becomes explicit instead of pidfile- and socket-poll-based.
- `config.yaml` remains human-inspectable and canonical.
- UUID-backed machine identity gives stable manager identity while allowing mutable names.
- Existing Negotiate behavior can be preserved while moving ownership to the right layers.

### Negative

- The refactor will touch multiple crate boundaries.
- Startup and shutdown become more explicitly structured and therefore require careful migration.
- SQLite introduces a new source of manager state that must stay consistent with the filesystem.
- Temporary adapters may be needed while moving logic out of `bento-runtime` and the CLI.

## Open Questions

- The exact SQLite schema is still to be defined.
- The exact startup pipe wire format still needs to be finalized.
- The exact forced-shutdown timeout and escalation behavior still needs to be finalized.
- The exact `VmMonitorService::Inspect` payload still needs to be defined.
- The future tunnel mode transport and daemon API surface remain deferred.
