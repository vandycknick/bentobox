# Coding Conventions

**Analysis Date:** 2026-02-08

## Naming Patterns

**Files:**
- Use snake_case file names for modules (examples: `src/api/routes.rs`, `src/core/bento_vmm.rs`, `src/bin/bento_cli.rs`).

**Functions:**
- Use snake_case for functions and methods (examples: `create_vm` in `src/api/routes.rs`, `install_macos` in `src/vm.rs`, `get_cache_dir` in `src/fs.rs`).

**Variables:**
- Use snake_case for locals and fields (examples: `aux_path` in `src/bin/bento_cli.rs`, `state_notifications` in `src/vm.rs`).

**Types:**
- Use PascalCase for structs/enums (examples: `VirtualMachineBuilder` in `src/vm.rs`, `IpswRegistry` in `src/downloader/ipsw.rs`).
- Use PascalCase enum variants (examples: `VirtualMachineState::Running` in `src/vm.rs`).
- Use explicit `#[allow(non_snake_case)]` or `#[allow(non_camel_case_types)]` where FFI requires it (examples: `src/internal.rs`, `src/dispatch/ffi.rs`).

## Code Style

**Formatting:**
- Use 4-space indentation and LF line endings per `.editorconfig`.
- Rust formatting config is not detected (no `rustfmt.toml`), so rely on default rustfmt conventions alongside `.editorconfig`.

**Linting:**
- Lint configuration not detected (no `clippy.toml` or other lint config in repo root).

## Import Organization

**Order:**
1. Standard library imports first (examples: `use std::{...};` in `src/vm.rs`, `src/bin/bento_cli.rs`).
2. External crates next (examples: `use axum::...;` in `src/api/routes.rs`, `use objc2::...;` in `src/vm.rs`).
3. Local crate modules last (examples: `use crate::...;` in `src/vm.rs`, `src/api/mod.rs`).

**Path Aliases:**
- No `use` alias patterns or module path aliases are configured in source files (examples: `src/lib.rs`, `src/dispatch/mod.rs`).

## Error Handling

**Patterns:**
- Use `anyhow::Result` in CLI flows and IO-heavy code (examples: `create_command` in `src/bin/bento_cli.rs`, `IpswRegistry::download` in `src/downloader/ipsw.rs`).
- Use `eyre::Result` in app runtime/daemon paths (examples: `Bentod::run` and `Bentod::create_server` in `src/bin/bentod/main.rs`, `BentoVirtualMachineManager::create` in `src/core/bento_vmm.rs`).
- Return explicit `Result<..., String>` for VM operations in `src/vm.rs`.
- `unwrap()` and `expect()` are used in runtime code paths (examples: `src/vm.rs`, `src/bin/bento_cli.rs`, `src/downloader/ipsw.rs`), follow the existing pattern when matching current behavior.

## Logging

**Framework:** console output via `println!`, `eprintln!`, and `dbg!`.

**Patterns:**
- Use `println!` for progress and state (examples: `src/vm.rs`, `src/bin/bento_cli.rs`).
- Use `eprintln!` for error paths (example: `src/bin/bentod/main.rs`).
- Use `dbg!` for debug-only observability in UI lifecycle code (example: `src/window.rs`).

## Comments

**When to Comment:**
- Use `// TODO:` and `// NOTE:` for known gaps or design caveats (examples: `src/vm.rs`, `src/bin/bento_cli.rs`).

**JSDoc/TSDoc:**
- Not applicable (Rust codebase). Use Rust doc comments `///` for API/FFI documentation (examples: `src/dispatch/queue.rs`, `src/internal.rs`).

## Function Design

**Size:**
- Large `impl` blocks are common for FFI bindings and builder patterns (examples: `src/internal.rs`, `src/vm.rs`).

**Parameters:**
- Use `&str` or `impl AsRef<str>` for string inputs (examples: `install_macos` and `use_platform_macos` in `src/vm.rs`).

**Return Values:**
- Return `Result` for fallible operations and `Option` for nullable data from FFI (examples: `VirtualMachine::start` in `src/vm.rs`, `get_cache_dir` in `src/fs.rs`).

## Module Design

**Exports:**
- Use `pub mod` declarations in `src/lib.rs` to expose top-level modules.
- Use `pub use` for targeted re-exports in submodules (examples: `pub use queue::Queue;` in `src/dispatch/mod.rs`).

**Barrel Files:**
- Limited to module glue like `src/dispatch/mod.rs`, no global barrel module detected.

---

*Convention analysis: 2026-02-08*
