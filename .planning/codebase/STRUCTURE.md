# Codebase Structure

**Analysis Date:** 2026-02-08

## Directory Layout

```
[project-root]/
├── src/                 # Rust library and binaries
├── boxos/               # Build assets for Linux guest/rootfs
├── docs/                # Project documentation
├── .planning/codebase/  # GSD codebase mapping output
├── Cargo.toml           # Rust workspace configuration
└── README.md            # Project overview
```

## Directory Purposes

**src/:**
- Purpose: Core Rust library and entry points.
- Contains: Modules (`src/lib.rs`), binaries (`src/bin/`), and platform bindings.
- Key files: `src/lib.rs`, `src/vm.rs`, `src/internal.rs`, `src/bin/bentod/main.rs`.

**src/api/:**
- Purpose: HTTP API router and handlers.
- Contains: Router setup and route handlers.
- Key files: `src/api/mod.rs`, `src/api/routes.rs`.

**src/core/:**
- Purpose: VM orchestration layer.
- Contains: `BentoVirtualMachineManager` implementation.
- Key files: `src/core/bento_vmm.rs`.

**src/dispatch/:**
- Purpose: Grand Central Dispatch wrapper for macOS.
- Contains: FFI bindings and queue wrapper.
- Key files: `src/dispatch/ffi.rs`, `src/dispatch/queue.rs`.

**src/downloader/:**
- Purpose: Restore image registry and download.
- Contains: IPSW registry and downloader.
- Key files: `src/downloader/ipsw.rs`.

**src/bin/:**
- Purpose: Binary entry points.
- Contains: CLI and daemon mains.
- Key files: `src/bin/bentod/main.rs`, `src/bin/bento_cli.rs`, `src/bin/linux.rs`, `src/bin/bentoctl/main.rs`.

**boxos/:**
- Purpose: Linux guest build artifacts and configs.
- Contains: Kernel configs, rootfs scripts, busybox and container files.
- Key files: `boxos/rootfs/build.sh`, `boxos/initramfs/init.sh`, `boxos/configs/linux-6.6.72-arm.config`.

**docs/:**
- Purpose: Project documentation.
- Contains: High-level architecture document.
- Key files: `docs/architecture.md`.

## Key File Locations

**Entry Points:**
- `src/bin/bentod/main.rs`: Axum daemon exposing `/api` routes.
- `src/bin/bento_cli.rs`: CLI for Linux guest VM start and terminal console.
- `src/bin/linux.rs`: CLI for macOS VM start and VNC setup.
- `src/bin/bentoctl/main.rs`: Placeholder CLI binary.

**Configuration:**
- `Cargo.toml`: Rust crate configuration.

**Core Logic:**
- `src/vm.rs`: VM build/runtime logic and state handling.
- `src/core/bento_vmm.rs`: Core VM manager used by API.
- `src/internal.rs`: objc2 Virtualization bindings.

**Testing:**
- Not detected in current tree.

## Naming Conventions

**Files:**
- `mod.rs` for module roots (example: `src/core/mod.rs`).
- `main.rs` for binary entry points (example: `src/bin/bentod/main.rs`).
- `snake_case.rs` for modules (example: `src/bin/bento_cli.rs` and `src/termios.rs`).

**Directories:**
- Lowercase module names (example: `src/api/`, `src/core/`, `src/dispatch/`).

## Where to Add New Code

**New Feature:**
- Primary code: `src/` module matching the feature, with exports from `src/lib.rs`.
- Tests: Not detected, add co-located tests under target modules (example: `src/vm.rs`).

**New Component/Module:**
- Implementation: Create a new submodule directory and `mod.rs` under `src/` (example: `src/new_feature/mod.rs`).

**Utilities:**
- Shared helpers: `src/utils.rs` or a new module under `src/` with export in `src/lib.rs`.

## Special Directories

**target/:**
- Purpose: Rust build outputs.
- Generated: Yes.
- Committed: No.

**.opencode/:**
- Purpose: Tooling metadata and agent templates.
- Generated: Yes.
- Committed: Yes.

---

*Structure analysis: 2026-02-08*
