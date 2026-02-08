# Technology Stack

**Analysis Date:** 2026-02-08

## Languages

**Primary:**
- Rust 2021 edition - Core application code in `Cargo.toml` and `src/`

**Secondary:**
- Makefile syntax - Build orchestration in `Makefile`
- Dockerfile syntax - Image builds in `boxos/Containerfile` and `boxos/rootfs/Dockerfile`

## Runtime

**Environment:**
- macOS (Apple frameworks via objc2) - Virtualization and UI bindings in `src/vm.rs` and `src/internal.rs`

**Package Manager:**
- Cargo (Rust) - `Cargo.toml`
- Lockfile: present (`Cargo.lock`)

## Frameworks

**Core:**
- Axum 0.8.1 - HTTP API server for the daemon in `src/bin/bentod/main.rs` and `src/api/mod.rs`
- Tokio 1.x - Async runtime in `src/bin/bentod/main.rs`

**Testing:**
- Not detected (no test framework configuration in `Cargo.toml`)

**Build/Dev:**
- Cargo - Rust build tooling in `Cargo.toml`
- Docker - Kernel/rootfs build pipeline in `Makefile`, `boxos/Containerfile`, and `boxos/rootfs/Dockerfile`

## Key Dependencies

**Critical:**
- objc2-virtualization 0.3.0 - Apple Virtualization.framework bindings in `src/vm.rs`
- objc2-app-kit 0.3.0 - AppKit integration in `src/window.rs`
- objc2-foundation 0.3.0 - Foundation bindings for paths and utilities in `src/fs.rs`
- reqwest 0.12.8 - HTTP download client in `src/downloader/ipsw.rs`

**Infrastructure:**
- clap 4.5.16 - CLI argument parsing in `src/bin/bentod/main.rs` and `src/bin/bento_cli.rs`
- sqlx 0.8.3 (sqlite) - Database client dependency declared in `Cargo.toml` (no usage found in `src/`)

## Configuration

**Environment:**
- No `.env` files detected; configuration uses CLI flags and hardcoded defaults in `src/bin/bentod/main.rs` and `src/downloader/ipsw.rs`
- Local paths derived from macOS system directories in `src/fs.rs`

**Build:**
- Rust manifests: `Cargo.toml`, `Cargo.lock`
- Build scripts: `Makefile`, `boxos/Containerfile`, `boxos/rootfs/Dockerfile`
- Codesigning entitlements: `app.entitlements`

## Platform Requirements

**Development:**
- macOS host required for Apple Virtualization.framework and AppKit bindings in `src/vm.rs` and `src/window.rs`
- Docker required for kernel/rootfs build steps in `Makefile`

**Production:**
- Not specified; runtime appears to be macOS-hosted CLI/daemon in `src/bin/bento_cli.rs` and `src/bin/bentod/main.rs`

---

*Stack analysis: 2026-02-08*
