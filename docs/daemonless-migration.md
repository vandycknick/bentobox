# Daemonless Migration Plan

## Purpose

This document is the implementation plan for the daemonless architecture described in `docs/adr/0004-daemonless-architecture.md`.

This plan is a living document.

Whenever implementation changes the intended sequencing, crate boundaries, API shape, filesystem layout, or runtime behavior, we must update:

- this plan, so the migration steps remain accurate,
- the ADR, so the architecture record reflects reality.

The ADR is the architectural source of truth.
This document is the execution plan and checklist.

## Ground Rules

- Keep changes incremental and shippable where possible.
- Prefer introducing new types and crates before moving behavior.
- Do not break the working CLI unless a phase explicitly replaces the old path.
- Do not leave undocumented architecture drift. If reality changes, update the ADR and this plan in the same change.
- Preserve the existing Negotiate protocol behavior while moving ownership to the correct crates.
- Keep one VM = one `vmmon` process as the target model throughout the migration.

## Target End State

At the end of the migration:

- `bentoctl` is a thin frontend over `bento-libvm`.
- `bento-core` owns the canonical shared domain model, including `VmSpec` and machine identity types.
- `bento-libvm` owns machine lifecycle, machine inventory, state layout, image/profile/bootstrap policy, Negotiate client logic, and spawning `bento-vmmon`.
- `bento-vmmon` owns per-VM supervision, Negotiate server logic, runtime state, serial/vsock/RPC upgrades, and signal-based shutdown.
- `bento-vmm` remains the backend abstraction.
- `config.yaml` in `~/.local/share/bento/instances/<ulid>/` is the canonical machine config.
- `~/.local/share/bento/state.redb` stores machine metadata and name/ULID mappings.

## Current High-level Migration Strategy

The migration is split into seven phases:

1. Define the architecture and migration plan.
2. Introduce `bento-core` and canonical shared types.
3. Introduce `bento-libvm` as the new engine crate.
4. Adopt ULID identity, `redb`, and the new on-disk layout.
5. Rename and restructure `bento-instanced` into `bento-vmmon`.
6. Split manager and monitor APIs, and move Negotiate ownership.
7. Thin the CLI and remove obsolete seams.

The phases are intentionally ordered to establish stable contracts before moving process ownership and runtime behavior.

## Tracking Conventions

Use these markers while working through the phases:

- `[ ]` not started
- `[-]` in progress
- `[x]` completed
- `[!]` blocked or partially changed and requires ADR/plan update

## Phase 1: Architecture and Plan

### Goal

Capture the target daemonless architecture and the migration sequence in-repo before code changes begin.

### Tasks

- [x] Write the daemonless architecture ADR.
- [x] Capture crate boundaries for `bentoctl`, `bento-core`, `bento-libvm`, `bento-vmmon`, and `bento-vmm`.
- [x] Document startup pipe synchronization.
- [x] Document signal-based stop semantics.
- [x] Document the on-disk layout and `redb` location.
- [x] Document `InstanceService` and `VmMonitorService` responsibilities.
- [x] Create this migration plan.

### Exit Criteria

- ADR exists and reflects the target architecture.
- Migration plan exists and is specific enough to guide implementation.

### Notes

- Phase 1 is complete.

## Phase 2: Introduce `bento-core`

### Goal

Create the canonical shared domain crate without moving lifecycle behavior yet.

### Why this phase exists

The refactor needs a stable domain contract before storage, process supervision, and crate ownership move around. `bento-core` is that contract.

### Deliverables

- New crate: `crates/bento-core`
- Canonical machine identity types
- Canonical `VmSpec`
- Supporting domain types used by `VmSpec`
- Serialization support for `config.yaml`

### Tasks

- [x] Add `crates/bento-core/Cargo.toml`.
- [x] Add `crates/bento-core/src/lib.rs`.
- [x] Add ULID-backed machine identity type.
- [x] Define the canonical `VmSpec`.
- [x] Define supporting types used by `VmSpec`.
- [x] Add serde derives and serialization support.
- [x] Add round-trip tests for `VmSpec` serialization.
- [ ] Add validation helpers only if clearly needed by multiple crates.
- [x] Add `bento-core` to the workspace members.

### Detailed Steps

1. Create the crate and add it to the workspace.
2. Add a machine identity abstraction backed by ULID.
3. Implement `VmSpec` and supporting enums/structs from the ADR.
4. Keep the API narrow. Avoid adding storage helpers, path helpers, or process code here.
5. Add serialization tests for `config.yaml` compatibility.
6. Update the ADR if the exact type shape or names change during implementation.
7. Update this plan if the scope of `bento-core` grows or shrinks.

### Exit Criteria

- `bento-core` compiles.
- `VmSpec` exists and is serializable.
- Machine identity type exists and is ULID-backed.
- No behavior has moved yet.

### Status

- `bento-core` has been added with `MachineId`, `VmSpec`, and supporting domain types.
- YAML round-trip tests are in place.
- Workspace-wide `cargo clippy --all --benches --tests --examples --all-features` currently fails on macOS in the existing `netlink-sys` dependency path pulled by other crates, not in `bento-core`.
- `cargo clippy -p bento-core --tests --all-features` passes.

### Risks

- Prematurely moving behavior into `bento-core` will blur boundaries.
- Reusing old runtime types without cleanup may create duplicate or conflicting models.

## Phase 3: Introduce `bento-libvm`

### Goal

Create the new engine crate and start moving manager-owned behavior into it.

### Why this phase exists

The CLI must eventually stop owning business logic, and runtime policy must move out of `bento-runtime`.

### Deliverables

- New crate: `crates/bento-libvm`
- Initial engine context and layout helpers
- Initial machine management surface
- Initial adapters so existing CLI flows can start routing through `bento-libvm`

### Tasks

- [x] Add `crates/bento-libvm/Cargo.toml`.
- [x] Add `crates/bento-libvm/src/lib.rs`.
- [x] Add top-level data-dir resolution helpers.
- [x] Add instance-dir path helpers for ULID-backed paths.
- [x] Add manager-facing error types.
- [x] Add machine lookup and identity resolution abstractions.
- [x] Add initial create/start/stop/list/inspect API skeletons.
- [ ] Start moving `bento-runtime` policy code into `bento-libvm`.
- [ ] Keep compatibility shims where needed so the workspace still builds.

### Detailed Steps

1. Create the crate and wire it into the workspace.
2. Add the basic library surface for machine engine operations.
3. Move path/layout concerns first, because those are easy to isolate.
4. Move machine inventory and manager logic next.
5. Leave direct `vmmon` spawning behavior for a later phase.
6. Avoid rewriting the CLI in the same step. Add a stable engine API first.
7. Update the ADR if actual `bento-libvm` scope differs from the planned boundary.
8. Update this plan if compatibility shims turn out to require an additional sub-phase.

### Candidate modules to move from `bento-runtime`

- machine and instance store logic
- directories and layout helpers
- profiles and host config policy
- image store orchestration
- global configuration access
- future client-side Negotiate ownership, later in the phase plan

### Exit Criteria

- `bento-libvm` exists and compiles.
- New code can start depending on `bento-libvm` for manager concerns.
- A meaningful slice of runtime policy has moved out of `bento-runtime`.

### Status

- `bento-libvm` has been added to the workspace.
- The first slice includes canonical data-dir and layout ownership, including `state.redb`, `instances/<ulid>/`, and `images/` path helpers.
- `MachineRef` now provides the initial manager-facing name-vs-ULID lookup abstraction.
- Manager-facing error types exist for layout resolution and machine-name validation.
- A first `LibVm` facade now exists with `create_pending`, `inspect`, and `list` methods.
- `LibVm` now owns canonical `VmSpec` config writing for new machines and is backed by `redb` metadata for create, inspect, list, and remove.
- `LibVm::start` now owns monitor spawning for the new path.
- `LibVm::stop` now owns monitor signaling and shutdown waiting for the new path.
- Deeper runtime policy migration out of `bento-runtime` is still pending in this phase.
- Startup synchronization now uses a startup pipe instead of pidfile polling.

### Risks

- Moving too much behavior at once will make failures hard to isolate.
- Introducing `bento-libvm` without a clear compatibility layer can break the current CLI mid-migration.

## Phase 4: Adopt ULID identity, `redb`, and new layout

### Goal

Move machine identity and manager state to the new canonical model.

### Why this phase exists

The architecture depends on stable machine identity, manager-owned metadata, and a canonical instance directory layout before `vmmon` can become data-dir driven.

### Deliverables

- `redb` database at `~/.local/share/bento/state.redb`
- ULID-based machine creation flow
- `~/.local/share/bento/instances/<ulid>/` layout
- Canonical `config.yaml` written from `bento-core::VmSpec`

### Tasks

- [x] Add `redb` dependency to the appropriate crate.
- [x] Define `redb` tables for machine identity and metadata.
- [x] Store ULID to name mapping.
- [x] Store name to ULID mapping.
- [x] Store instance directory path.
- [ ] Store creation time, labels, restart policy, and related metadata.
- [x] Change machine creation to allocate ULIDs.
- [x] Change machine creation to create `instances/<ulid>/`.
- [x] Write `config.yaml` as the canonical machine config.
- [ ] Add migration or compatibility behavior for existing name-based instance directories if needed.

### Detailed Steps

1. Add the `redb` schema for the minimum required manager records.
2. Add a ULID allocation path during machine creation.
3. Resolve machine names through the database instead of assuming directory names are names.
4. Create `instances/<ulid>/` directories.
5. Write `config.yaml` from `bento-core::VmSpec`.
6. Decide whether existing machines need migration, lazy compatibility lookup, or a one-time import path.
7. Update the ADR if compatibility behavior changes the long-term model.
8. Update this plan if legacy migration turns out to require its own dedicated phase.

### Exit Criteria

- New machines are created with ULIDs.
- New machines use `instances/<ulid>/config.yaml`.
- `redb` stores the canonical name and ULID mappings.
- Manager lookup no longer depends on machine names as directory names.

### Status

- `bento-libvm` now creates new machines with ULIDs.
- New machine configs are written as canonical `VmSpec` YAML under `instances/<ulid>/config.yaml`.
- `redb` now stores ULID-to-name, name-to-ULID, and instance-dir mappings for the new path.
- Additional machine metadata such as creation time, labels, and restart policy are still pending.
- Legacy instance migration behavior is still intentionally undefined.

### Risks

- Existing machine compatibility may be trickier than expected.
- Partial writes during create must be handled transactionally enough to avoid orphaned directories or DB records.

## Phase 4.5: One-off local migration tool

### Goal

Provide a one-off local tool that migrates existing old-world VMs into the new ULID, `VmSpec`, and `redb`-backed layout.

### Why this phase exists

The new architecture no longer treats the old name-based instance layout as a long-term compatibility target. Existing machines therefore need a one-time migration path rather than a permanent compatibility layer.

### Deliverables

- a one-off local migration tool in the workspace
- dry-run support
- per-VM or all-VM execution
- explicit refusal to migrate running VMs

### Tasks

- [x] Add a one-off migration tool, likely as a small Rust workspace crate or internal bin.
- [x] Discover old-world instance directories.
- [x] Parse old `InstanceConfig` from old-world `config.yaml`.
- [x] Convert old config into canonical `VmSpec`.
- [x] Allocate a new ULID for each migrated VM.
- [x] Move each VM into `~/.local/share/bento/instances/<ulid>/`.
- [x] Rewrite `config.yaml` as canonical `VmSpec`.
- [x] Insert name and ULID mappings into `~/.local/share/bento/state.redb`.
- [x] Refuse to migrate any VM whose `id.pid` points to a live process.
- [x] Support dry-run output before making changes.

### Status

- A one-off migration tool crate now exists at `crates/bento-migrate-daemonless`.
- The tool supports dry-run by default and `--execute` to apply migrations.
- The tool supports `--all` or explicit VM names.
- The tool refuses any VM whose `id.pid` points to a live process.
- The tool migrates old-world configs into canonical `VmSpec`, moves the directory into the ULID-backed layout, and registers the migrated VM in `redb`.

### Exit Criteria

- Existing stopped VMs can be migrated into the new layout.
- Running VMs are refused explicitly and safely.
- The tool is good enough for one-time local use and does not introduce a new compatibility layer.

## Phase 5: Rename and restructure `bento-instanced` into `bento-vmmon`

### Goal

Turn the current monitor into a proper per-VM supervisor with self-daemonization and data-dir-driven startup.

### Why this phase exists

Today the CLI still starts the monitor and the monitor still owns too much mixed behavior. This phase gives the monitor a clear runtime-only boundary.

### Deliverables

- crate rename from `bento-instanced` to `bento-vmmon`
- binary rename to `vmmon`
- `--data-dir` driven startup
- single `main.rs` entrypoint with separate bootstrap and `run()` responsibilities
- self-daemonization support
- startup pipe handshake
- signal-based shutdown path

### Tasks

- [x] Rename crate `bento-instanced` to `bento-vmmon`.
- [ ] Rename public references and imports.
- [ ] Reorganize the code into `main`, `run`, `startup`, `services`, `shutdown`, `state`, `context`, and `supervisor` modules.
- [x] Add `--data-dir` argument.
- [x] Load `config.yaml` from the passed instance directory.
- [ ] Move process daemonization into `vmmon`.
- [ ] Add a foreground mode for tests and debugging.
- [x] Add startup pipe support.
- [x] Report structured startup success or failure over the startup pipe.
- [ ] Add signal handlers for graceful stop.
- [ ] Add forced-stop escalation behavior.
- [ ] Persist exit state in the instance directory.

### Detailed Steps

1. Rename the crate and binary first, before large behavior moves, so subsequent work lands on the right names.
2. Reorganize the monitor entrypoint into a single `main.rs` with bootstrap setup and a separate `run()` function.
3. Change monitor startup to accept a concrete `--data-dir` instead of name-driven lookup.
4. Move config loading and runtime artifact setup into startup code.
5. Add the startup pipe handshake.
6. Add explicit signal-driven stop behavior.
7. Keep existing functionality working while the internal structure changes.
8. Update the ADR if startup success semantics, shutdown semantics, or module boundaries differ from the planned model.
9. Update this plan if `vmmon` needs an additional internal phase split.

### Exit Criteria

- `bento-vmmon` exists and runs from `--data-dir`.
- `bento-vmmon` can daemonize itself.
- `bento-vmmon` can report startup success or failure through a pipe.
- `bento-vmmon` can be stopped through signals.

### Status

- The package and Rust crate have been renamed to `bento-vmmon` / `bento_vmmon`.
- A real `bento-vmmon` binary now exists with a single `main.rs` entrypoint that separates bootstrap from `run()`.
- The generated monitor executable is now named `vmmon`.
- `bentoctl` now launches the hidden `vmmon` subcommand, with `instanced` retained as a hidden alias during the transition.
- `vmmon` now accepts `--data-dir` as its startup contract and no longer has a legacy `--name` monitor path.
- `bento-vmmon` now reads `config.yaml` from the instance directory and drives the data-dir path directly from `VmSpec`.
- `bento-vmmon` now reports startup success or failure back to `bento-libvm` over a startup pipe.
- `MonitorConfig` has been collapsed into a single `VmContext` for the data-dir-driven monitor path.
- The old `VmSpec -> InstanceConfig` adapter has been deleted from `bento-vmmon`.
- The on-disk crate directory is still `crates/bento-instanced/` for now to keep the diff narrow while the internal restructuring continues.

### Risks

- Renaming and restructuring at the same time can create noisy diffs.
- Startup semantics are easy to get subtly wrong if pidfiles, sockets, and backend startup are not sequenced carefully.

## Phase 6: Split manager and monitor APIs, move Negotiate ownership

### Goal

Separate manager APIs from per-VM monitor APIs and move Negotiate ownership to the correct layers.

### Why this phase exists

The long-term architecture depends on a stable distinction between machine-manager operations and per-VM runtime operations.

### Deliverables

- manager `InstanceService`
- per-VM `VmMonitorService`
- Negotiate server owned by `bento-vmmon`
- Negotiate client owned by `bento-libvm`

### Tasks

- [ ] Define or update protocol definitions for `InstanceService`.
- [ ] Define or update protocol definitions for `VmMonitorService`.
- [ ] Ensure `VmMonitorService` does not include `Stop`.
- [ ] Move monitor RPC server implementation into `bento-vmmon` under the new service boundary.
- [ ] Move client-side monitor connection logic from `bentoctl` into `bento-libvm`.
- [ ] Keep existing Negotiate behavior for serial attach, vsock connect, and RPC upgrades.
- [ ] Remove direct CLI ownership of Negotiate clients.

### Detailed Steps

1. Define the protocol split in `bento-protocol`.
2. Add or adapt service implementations in `bento-vmmon`.
3. Move client-side upgrade logic into `bento-libvm`.
4. Update CLI call sites to use `bento-libvm` APIs rather than raw monitor connections.
5. Preserve compatibility as much as possible while migrating service names and ownership.
6. Update the ADR if exact service method sets or upgrade flows change.
7. Update this plan if the protocol migration requires a compatibility phase.

### Exit Criteria

- Manager-facing APIs and monitor-facing APIs are clearly separated.
- CLI no longer directly owns monitor protocol behavior.
- Negotiate ownership matches the daemonless architecture.

### Risks

- Protocol churn can ripple through the CLI, monitor, and guest-facing flows.
- If the split is too abrupt, migration may temporarily break status, shell, or exec flows.

## Phase 7: Thin the CLI and remove obsolete seams

### Goal

Complete the architecture shift by removing old ownership from the CLI and shrinking or deleting obsolete runtime code.

### Why this phase exists

The migration is not complete until the CLI is thin and the old ownership paths are gone.

### Deliverables

- CLI lifecycle commands routed through `bento-libvm`
- removal of direct CLI monitor spawning
- removal of CLI-owned pidfile polling startup logic
- removal or shrinking of obsolete `bento-runtime` code

### Tasks

- [ ] Route CLI create/start/stop/list/status/shell/exec flows through `bento-libvm`.
- [ ] Remove hidden or direct monitor start commands from the CLI if no longer needed.
- [ ] Remove direct CLI process spawning of the monitor.
- [ ] Remove old pidfile and socket polling startup logic from the CLI.
- [ ] Remove obsolete `bento-runtime` modules that were moved into `bento-libvm` or `bento-core`.
- [ ] Clean up imports, old helpers, and dead code.
- [ ] Update user docs if command behavior or output changes.

### Detailed Steps

1. Switch each CLI command one at a time to `bento-libvm`.
2. Remove now-dead helpers in the CLI.
3. Remove or collapse the remains of `bento-runtime`.
4. Delete compatibility shims once the new path is fully exercised.
5. Update the ADR if the final ownership differs from the original proposal.
6. Update this plan to mark the phase complete and note any deferred follow-up.

### Exit Criteria

- The CLI is a thin frontend.
- `bento-libvm` owns manager behavior.
- `bento-vmmon` owns runtime supervision.
- `bento-runtime` no longer exists or no longer owns manager/runtime business logic.

### Status

- `bentoctl list` now reads machines through `bento-libvm`.
- `bentoctl delete` now removes machines through `bento-libvm`.
- `bentoctl create` now routes through `bento-libvm` and creates machines directly in the ULID, `VmSpec`, and `redb`-backed layout.
- `bentoctl create-raw` now routes through `bento-libvm` and creates raw machines directly in the new layout.
- `bentoctl start` now routes through `bento-libvm`, which spawns `vmmon --data-dir ...`.
- `bentoctl stop` now routes through `bento-libvm`, which signals `vmmon` by pidfile for the new path.
- `status`, `shell`, and `exec` still depend on the old runtime and monitor path for now because they are tied to the current monitor socket contract.
- CLI-owned pidfile polling for startup has been removed from the new path.

### Risks

- Dead code may linger if cleanup is deferred too long.
- CLI behavior may still accidentally depend on old runtime assumptions.

## Cross-cutting Workstreams

These are not standalone phases, but they must be handled throughout the migration.

### Documentation

- [ ] Update the ADR whenever architecture changes.
- [ ] Update this plan whenever sequencing or scope changes.
- [ ] Update user-facing docs when command or storage behavior changes.

### Testing

- [ ] Add tests for `VmSpec` serialization and config compatibility.
- [ ] Add tests for ULID-backed lookup and state persistence.
- [ ] Add tests for startup pipe success and failure paths.
- [ ] Add tests for signal-driven stop behavior.
- [ ] Add tests for Negotiate upgrades after ownership moves.

### Compatibility and migration

- [x] Decide how existing name-based instance directories are discovered or migrated.
- [x] Decide whether manager state can be lazily reconstructed from disk for legacy instances.
- [ ] Decide the exact cutover point where old CLI ownership paths are removed.

### Error handling and observability

- [ ] Ensure `vmmon` startup failures report back to `libvm` clearly.
- [ ] Ensure logs exist for startup, shutdown, backend failure, and escalation paths.
- [ ] Ensure exit state is durable and inspectable.

## Suggested Immediate Next Steps

The next implementation slice should be:

1. Add the one-off local migration tool for existing old-world VMs.
2. Move monitor connection and Negotiate client ownership into `bento-libvm`.
3. Route `status`, `shell`, and `exec` through `bento-libvm`.

That closes the remaining gap between the new manager/monitor lifecycle and the old CLI-owned monitor protocol path.

## Change Log

Add dated notes here whenever the migration plan changes materially.

- 2026-04-06: Initial migration plan created.
- 2026-04-06: Phase 2 started, `bento-core` added with ULID-backed `MachineId` and initial `VmSpec` model.
- 2026-04-06: Phase 3 started, `bento-libvm` added with layout helpers, `MachineRef`, and initial manager-facing error types.
- 2026-04-06: Phase 3 progressed, `LibVm` facade added for create/list/inspect flows.
- 2026-04-06: Phase 4 started, `bento-libvm` adopted `redb`, ULID-backed machine creation, and canonical `VmSpec` config writing.
- 2026-04-06: Phase 7 started incrementally, `bentoctl list` and `bentoctl delete` now route through `bento-libvm`.
- 2026-04-06: Phase 5 started, monitor package/crate renamed to `bento-vmmon` and split into thin `main` plus `run()` entrypoint.
- 2026-04-06: Phase 5 progressed, `vmmon --data-dir` now loads canonical `VmSpec` from the instance directory.
- 2026-04-06: Phase 5 progressed, `vmmon` startup now reports `Started` and `Failed` over a startup pipe.
- 2026-04-06: Phase 5 progressed, `vmmon` data-dir path now consumes `VmSpec` directly and the temporary `VmSpec -> InstanceConfig` adapter was removed.
- 2026-04-06: Phase 5 progressed, `vmmon` no longer supports legacy `--name` startup and `MonitorConfig` was collapsed into `VmContext`.
- 2026-04-06: Phase 7 progressed, `bentoctl create` and `create-raw` now route through `bento-libvm`, so new machines are created directly in the new layout.
- 2026-04-06: Phase 7 progressed, `bentoctl start` now routes through `bento-libvm`, which owns `vmmon` spawning for the new path.
- 2026-04-06: Phase 7 progressed, `bentoctl stop` now routes through `bento-libvm`, and the old CLI daemon-control module was removed.
- 2026-04-06: Phase 4.5 started, `bento-migrate-daemonless` was added as a one-off local migration tool for old-world VMs.
