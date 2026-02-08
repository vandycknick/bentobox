# External Integrations

**Analysis Date:** 2026-02-08

## APIs & External Services

**OS/Platform Services:**
- Apple Virtualization.framework - VM lifecycle integration via objc2 bindings in `src/vm.rs` and `src/internal.rs`
  - SDK/Client: `objc2-virtualization` (declared in `Cargo.toml`)
  - Auth: Not applicable

**Download/Update Services:**
- Apple Software Update CDN (updates.cdn-apple.com) - IPSW downloads in `src/downloader/ipsw.rs`
  - SDK/Client: `reqwest` (declared in `Cargo.toml`)
  - Auth: Not detected

## Data Storage

**Databases:**
- Not detected (no database connections in `src/`; `src/data/mod.rs` is empty)
  - Connection: Not applicable
  - Client: `sqlx` dependency declared in `Cargo.toml`

**File Storage:**
- Local filesystem only; caches and application support paths resolved via macOS APIs in `src/fs.rs`

**Caching:**
- Local filesystem cache directory in `src/fs.rs` and `src/downloader/ipsw.rs`

## Authentication & Identity

**Auth Provider:**
- None detected; daemon API binds to a local Unix socket in `src/bin/bentod/main.rs`
  - Implementation: Local socket, no auth layer defined

## Monitoring & Observability

**Error Tracking:**
- None detected (no telemetry or error tracking clients in `src/`)

**Logs:**
- Standard error output via `eprintln!` in `src/bin/bentod/main.rs`

## CI/CD & Deployment

**Hosting:**
- Not specified in repository files

**CI Pipeline:**
- None detected (no workflows in `.github/workflows/`)

## Environment Configuration

**Required env vars:**
- Not detected (no `std::env` usage in `src/`)

**Secrets location:**
- Not detected

## Webhooks & Callbacks

**Incoming:**
- None detected

**Outgoing:**
- None detected

---

*Integration audit: 2026-02-08*
