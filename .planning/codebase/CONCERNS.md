# Codebase Concerns

**Analysis Date:** 2026-02-08

## Tech Debt

**Virtual machine config validation is ignored:**
- Issue: `VirtualMachineBuilder::build` logs validation errors but still constructs a VM.
- Files: `src/vm.rs`
- Impact: Invalid configurations proceed, leading to undefined behavior or crashes at runtime.
- Fix approach: Return `Result<VirtualMachine, Error>` and stop construction when validation fails.

**Placeholder paths in VM manager:**
- Issue: `BentoVirtualMachineManager::create` uses empty `aux_path` and `image_path` strings.
- Files: `src/core/bento_vmm.rs`
- Impact: VM creation fails or creates invalid configurations.
- Fix approach: Wire real paths from config or arguments, validate existence before build.

**Hard-coded IPSW registry and racey download flow:**
- Issue: IPSW list is static and the download path check is race-prone.
- Files: `src/downloader/ipsw.rs`
- Impact: Registry goes stale quickly, concurrent runs can corrupt or re-download unexpectedly.
- Fix approach: Move registry to config or remote index, add file locks and checksum verification.

**Empty/stub modules:**
- Issue: Config/data/models modules are empty and referenced for structure only.
- Files: `src/config/mod.rs`, `src/data/mod.rs`, `src/api/models.rs`
- Impact: Missing persistence/model layer, higher chance of ad hoc state or repeated logic.
- Fix approach: Define real config and data models, wire them into API and CLI workflows.

## Known Bugs

**Incorrect QoS mapping:**
- Symptoms: Utility QoS maps to user-initiated, which skews scheduling priority.
- Files: `src/dispatch/queue.rs`
- Trigger: `DispatchQoSClass::Utility` used to schedule work.
- Workaround: Replace with `QOS_CLASS_UTILITY` mapping.

**Linux CLI relies on developer-specific paths:**
- Symptoms: VM boot fails unless paths match the author’s local directory.
- Files: `src/bin/linux.rs`
- Trigger: Running on any machine without `/Users/nickvd/Projects/bentobox/target/boxos/...`.
- Workaround: Make kernel/initramfs/disk paths CLI args or config entries.

**VM manager create path failure:**
- Symptoms: API VM create returns success but underlying VM build fails.
- Files: `src/core/bento_vmm.rs`, `src/api/routes.rs`
- Trigger: `POST /api/vm` with current implementation.
- Workaround: Validate paths and propagate build errors to the API response.

## Security Considerations

**Hard-coded VNC password and exposed connection string:**
- Risk: Anyone with local access can use the static password; printed URL leaks credentials.
- Files: `src/bin/bento_cli.rs`
- Current mitigation: None detected.
- Recommendations: Generate per-VM secrets, load from config/secure store, avoid printing credentials.

**Unverified IPSW downloads:**
- Risk: No checksum/signature verification for downloaded restore images.
- Files: `src/downloader/ipsw.rs`
- Current mitigation: TLS only.
- Recommendations: Validate hashes/signatures before use, store expected checksums in registry.

**API has no authentication or authorization:**
- Risk: Any local process with socket access can create VMs.
- Files: `src/bin/bentod/main.rs`, `src/api/routes.rs`
- Current mitigation: Unix socket scope only.
- Recommendations: Add token-based auth or OS-level permission checks on the socket.

## Performance Bottlenecks

**Busy-wait for VNC port:**
- Problem: Tight loop polling VNC port without sleep.
- Files: `src/bin/bento_cli.rs`
- Cause: `loop { if port != 0 { ... } }` without backoff.
- Improvement path: Add sleep/backoff or use a readiness notification.

**Blocking IPSW downloads in the main thread:**
- Problem: `reqwest::blocking::get` runs synchronously, tying up the caller.
- Files: `src/downloader/ipsw.rs`
- Cause: Blocking API used for download and copy.
- Improvement path: Use async downloads or move to a worker thread with progress reporting.

## Fragile Areas

**Unsafe FFI and thread-safety TODOs:**
- Files: `src/vm.rs`, `src/observers.rs`, `src/dispatch/queue.rs`
- Why fragile: `unsafe` blocks and `unsafe impl Send/Sync` rely on assumptions not enforced by types.
- Safe modification: Add explicit safety invariants, wrap unsafe values in safe abstractions, avoid `unwrap` in observers.
- Test coverage: No safety-focused tests detected.

**Terminal raw mode not guaranteed to restore:**
- Files: `src/bin/linux.rs`, `src/termios.rs`
- Why fragile: Panic or early returns can leave the terminal in raw mode.
- Safe modification: Use a guard type with `Drop` to restore terminal state.
- Test coverage: No tests for terminal lifecycle.

## Scaling Limits

**Not detected:**
- Current capacity: Not detected in `src/`.
- Limit: Not detected in `Cargo.toml`.
- Scaling path: Not applicable.

## Dependencies at Risk

**Beta dependency in core build:**
- Risk: `futures-io = "0.2.0-beta"` may change API/behavior.
- Impact: Upgrades or compatibility issues with future Rust versions.
- Migration plan: Replace with stable `futures-io` release or eliminate dependency.
- Files: `Cargo.toml`

## Missing Critical Features

**Persistent config for machine identifiers:**
- Problem: Machine ID is printed but not stored or reloaded reliably.
- Blocks: Reliable VM restart/reuse without manual re-entry.
- Files: `src/vm.rs`

**API does not honor requested VM parameters:**
- Problem: `CreateVirtualMachine` payload fields are unused.
- Blocks: API-driven VM sizing and distro selection.
- Files: `src/api/routes.rs`

## Test Coverage Gaps

**No automated tests detected:**
- What's not tested: VM lifecycle, downloader integrity, API handlers, terminal behavior.
- Files: `src/lib.rs`, `src/`, `Cargo.toml`
- Risk: Regressions and unsafe behavior can ship without detection.
- Priority: High

---

*Concerns audit: 2026-02-08*
