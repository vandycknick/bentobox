# Architecture

**Analysis Date:** 2026-02-08

## Pattern Overview

**Overall:** Modular Rust library with multiple binary entry points, integrating macOS Virtualization.framework via objc2 FFI.

**Key Characteristics:**
- Library-centric modules are exported from `src/lib.rs` and consumed by binaries in `src/bin/bentod/main.rs`, `src/bin/bento_cli.rs`, `src/bin/linux.rs`, and `src/bin/bentoctl/main.rs`.
- Platform bindings and system integration live in dedicated modules like `src/internal.rs`, `src/dispatch/mod.rs`, and `src/vm.rs`.
- API surface uses Axum routing in `src/api/mod.rs` and `src/api/routes.rs` with shared state injected into handlers.

## Layers

**CLI/Process Entry Layer:**
- Purpose: Define runnable binaries and CLI commands.
- Location: `src/bin/`
- Contains: CLI parsing, runtime setup, and orchestration.
- Depends on: Library modules in `src/lib.rs`, virtualization APIs in `src/vm.rs`, and helpers in `src/fs.rs` and `src/utils.rs`.
- Used by: End users running `bento_cli`, `linux`, `bentod`, and `bentoctl` binaries (`src/bin/bento_cli.rs`, `src/bin/linux.rs`, `src/bin/bentod/main.rs`, `src/bin/bentoctl/main.rs`).

**API Layer:**
- Purpose: Provide HTTP API endpoints for VM lifecycle actions.
- Location: `src/api/`
- Contains: Router setup and request handlers (`src/api/mod.rs`, `src/api/routes.rs`).
- Depends on: Core VM manager in `src/core/bento_vmm.rs`.
- Used by: Daemon entry point in `src/bin/bentod/main.rs`.

**Core VM Management Layer:**
- Purpose: Encapsulate VM creation orchestration.
- Location: `src/core/`
- Contains: `BentoVirtualMachineManager` (`src/core/bento_vmm.rs`).
- Depends on: VM builder APIs in `src/vm.rs`.
- Used by: API handlers in `src/api/routes.rs`.

**VM Construction and Runtime Layer:**
- Purpose: Build and control VMs using Virtualization.framework.
- Location: `src/vm.rs`
- Contains: `VirtualMachine`, `VirtualMachineBuilder`, `VirtualMachineState`, platform setup, and state observation.
- Depends on: objc2 Virtualization bindings in `src/internal.rs`, dispatch queue wrapper in `src/dispatch/mod.rs`, observers in `src/observers.rs`, and UI integration in `src/window.rs`.
- Used by: CLI binaries (`src/bin/bento_cli.rs`, `src/bin/linux.rs`), core manager (`src/core/bento_vmm.rs`).

**Platform Integration Layer:**
- Purpose: Provide low-level bindings for macOS system APIs.
- Location: `src/internal.rs`, `src/dispatch/mod.rs`, `src/observers.rs`, `src/window.rs`, `src/termios.rs`, `src/fs.rs`.
- Contains: objc2 extern classes, GCD queue wrappers, KVO observers, AppKit window setup, terminal mode helpers, and macOS filesystem directories.
- Depends on: macOS frameworks via objc2 (`src/internal.rs`, `src/window.rs`, `src/observers.rs`).
- Used by: VM runtime (`src/vm.rs`) and CLI tools (`src/bin/bento_cli.rs`, `src/bin/linux.rs`).

## Data Flow

**HTTP VM Create Flow:**

1. Axum router receives `POST /api/vm` in `src/api/mod.rs` and routes to handler in `src/api/routes.rs`.
2. Handler calls `BentoVirtualMachineManager::create()` in `src/core/bento_vmm.rs`.
3. Manager builds a VM via `VirtualMachineBuilder` in `src/vm.rs` and returns a result.

**CLI VM Start Flow (Linux guest):**

1. CLI constructs VM parameters in `src/bin/linux.rs`.
2. `VirtualMachineBuilder` configures devices and builds in `src/vm.rs`.
3. `VirtualMachine::start()` triggers virtualization start using objc2 bindings in `src/internal.rs`.

**State Management:**
- In-memory state is carried in `AppState` in `src/api/mod.rs`, and VM state notifications are passed via channels in `src/vm.rs`.

## Key Abstractions

**VirtualMachine:**
- Purpose: Runtime VM control surface (start/stop/state).
- Examples: `src/vm.rs`.
- Pattern: Thin wrapper around objc2 Virtualization APIs with dispatch-queue execution.

**VirtualMachineBuilder:**
- Purpose: Fluent builder for configuring VM resources and devices.
- Examples: `src/vm.rs`.
- Pattern: Builder pattern with chained configuration methods.

**BentoVirtualMachineManager:**
- Purpose: High-level VM orchestration entry for API.
- Examples: `src/core/bento_vmm.rs`.
- Pattern: Stateless manager that constructs `VirtualMachineBuilder` pipelines.

**Dispatch Queue Wrapper:**
- Purpose: Provide safe-ish Rust API for GCD queues.
- Examples: `src/dispatch/queue.rs`, `src/dispatch/mod.rs`.
- Pattern: RAII wrapper with FFI shims and queue attributes.

**objc2 Virtualization Bindings:**
- Purpose: Bind to Virtualization.framework classes not provided by upstream crates.
- Examples: `src/internal.rs`.
- Pattern: `extern_class!` and `extern_methods!` wrappers.

## Entry Points

**bentod (daemon API):**
- Location: `src/bin/bentod/main.rs`
- Triggers: `bentod` binary execution.
- Responsibilities: Spin up Axum server over Unix socket and mount API routes.

**bento_cli (VM control CLI):**
- Location: `src/bin/bento_cli.rs`
- Triggers: `bento_cli` binary execution.
- Responsibilities: Build and start Linux VMs with local terminal console.

**linux (Linux VM helper):**
- Location: `src/bin/linux.rs`
- Triggers: `linux` binary execution.
- Responsibilities: Build and start macOS VMs, manage VNC server and state loop.

**bentoctl (placeholder):**
- Location: `src/bin/bentoctl/main.rs`
- Triggers: `bentoctl` binary execution.
- Responsibilities: Currently empty stub.

## Error Handling

**Strategy:** Mix of `Result` returns and direct `unwrap`/`expect` in entry points.

**Patterns:**
- `eyre::Result` and `anyhow::Result` at boundary layers (`src/bin/bentod/main.rs`, `src/bin/bento_cli.rs`, `src/downloader/ipsw.rs`).
- String-based errors in VM control paths (`src/vm.rs`).

## Cross-Cutting Concerns

**Logging:** `println!`/`eprintln!` used for operational output in `src/vm.rs`, `src/bin/linux.rs`, and `src/bin/bentod/main.rs`.
**Validation:** Input validation is minimal, mostly in CLI parsing (`src/bin/linux.rs`, `src/bin/bento_cli.rs`).
**Authentication:** No authentication layer is present in the API server in `src/api/mod.rs` and `src/api/routes.rs`.

---

*Architecture analysis: 2026-02-08*
