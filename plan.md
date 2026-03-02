# Bento Instance Guest Rollout Plan

Status: Draft v2  
Owner: bento  
Date: 2026-02-27

## Scope

Build guest-side service discovery in phases with low risk and clear boundaries.

- Phase 1 ships guest discovery and SSH endpoint advertisement.
- `instanced` stays synchronous in phase 1.
- Use tarpc for instance-to-guest discovery RPC.
- Keep guest control port an internal implementation detail, no instance config field and no CLI arg.
- Use kernel command-line propagation for host-to-guest control-port negotiation.
- Add CIDATA injection so guest agent is installed and started with systemd at boot.

---

## Architecture Snapshot

### Control Plane
- Transport: vsock
- Protocol: tarpc
- Service: `GuestDiscovery`
- Methods:
  - `list_services() -> Vec<ServiceEndpoint>`
  - `resolve_service(name: String) -> Option<ServiceEndpoint>`
  - `health() -> HealthStatus`

### Data Plane
- SSH traffic is proxied over a dedicated guest-selected vsock service port.
- Guest selects a random available service port from `2000..=8000`.

### Control Port Negotiation
- Host boot cmdline includes `bento.guest.control_port=<port>`.
- Guest reads actual kernel boot args from `/proc/cmdline`.
- Fallback default if key is missing or invalid: `1027`.

---

## Phase 1 (Implementation now)

Goal: Introduce guest discovery and SSH endpoint advertisement while keeping `instanced` sync, plus CIDATA/systemd bootstrap for the guest binary.

### 1) Shared protocol cleanup in runtime

Files:
- `crates/bento-runtime/src/guest_rpc.rs`
- `crates/bento-runtime/src/lib.rs` (export check)

Actions:
1. Rename `ServiceEndpoint.vsock_port` to `ServiceEndpoint.port`.
2. Keep `HealthStatus { ok: bool }` and `GuestDiscovery` tarpc trait.
3. Define or confirm constants:
   - `DEFAULT_DISCOVERY_PORT: u32 = 1027`
   - `GUEST_SERVICE_PORT_MIN: u32 = 2000`
   - `GUEST_SERVICE_PORT_MAX: u32 = 8000`
   - `KERNEL_PARAM_DISCOVERY_PORT: &str = "bento.guest.control_port"`

Acceptance:
- Protocol compiles and is shared by host and guest crates.

---

### 2) Guest daemon behavior (`bento-instance-guest`)

File:
- `crates/bento-instance-guest/src/main.rs`

Actions:
1. Add parser for `/proc/cmdline`:
   - scan tokens for `bento.guest.control_port=...`
   - validate `1..=65535`
   - fallback `DEFAULT_DISCOVERY_PORT`
2. Bind tarpc discovery server on parsed control port.
3. Keep SSH relay startup:
    - allocate random free vsock port in `2000..=8000`
    - retry on `AddrInUse`
    - fail clearly after bounded retries
    - use `tokio-vsock` so discovery and relay accept loops are async
4. Publish one service endpoint:
    - `ServiceEndpoint { name: "ssh", port: allocated_port }`
5. Implement RPC handlers:
   - `list_services`
   - `resolve_service`
   - `health`

Acceptance:
- Guest starts discovery listener and returns SSH service endpoint.

---

### 3) Instanced integration (sync with async discovery island)

File:
- `crates/bento-runtime/src/instance_daemon.rs`

Actions:
1. Keep daemon and control socket flow synchronous.
2. In discovery path:
   - open discovery vsock device
   - create short-lived tokio runtime
   - `block_on` tarpc calls (`health`, `list_services`)
3. Map discovered endpoints into registry using `ServiceEndpoint.port`.
4. Keep serial service behavior unchanged.

Acceptance:
- `ListServices` includes discovered SSH.
- `OpenService("ssh")` resolves and tunnels through discovered `port`.

---

### 4) Kernel cmdline wiring (host boot path)

File:
- `crates/bento-runtime/src/driver/vz/mod.rs`

Actions:
1. Build boot command line from a helper instead of one hardcoded string.
2. Append `bento.guest.control_port=<internal_default_or_resolved_value>`.
3. Keep existing kernel args intact.

Acceptance:
- VM boots with expected control-port kernel argument.

---

### 5) CIDATA guest-agent injection and systemd bootstrap

Files:
- `crates/bento-runtime/src/cidata.rs`
- `crates/bento-runtime/src/cidata_iso9660.rs` (likely unchanged API, just extra entries)
- `crates/bento-runtime/src/lib.rs`
- new: `crates/bento-runtime/src/global_config.rs`

Actions:
1. Add global config loader for `~/.config/bento/config.yaml`.
2. Initial config shape:

   ```yaml
   guest:
     agent_binary: "/absolute/path/to/bento-instance-guest"
   ```

3. During `build_cidata_iso(...)`:
   - read configured binary path
   - add binary bytes to CIDATA entries
   - extend cloud-init user-data with systemd unit install and start
4. Cloud-init must:
   - place binary at `/usr/local/bin/bento-instance-guest`
   - `chmod 0755`
   - write `/etc/systemd/system/bento-instance-guest.service`
   - run `systemctl daemon-reload`
   - run `systemctl enable --now bento-instance-guest.service`

Failure handling for phase 1:
- If `guest.agent_binary` is missing or invalid, fail fast with clear error.

Acceptance:
- Agent binary arrives in VM at first boot and is running under systemd.

---

### 6) Tests for phase 1

Add or update targeted tests:

1. Guest cmdline parser:
   - valid key parses
   - missing key falls back to `1027`
   - invalid value falls back to `1027`
2. Guest service-port allocator:
   - selected port in `2000..=8000`
   - retries on collisions
3. Protocol serde:
   - `ServiceEndpoint { name, port }` round-trip
   - `HealthStatus` round-trip
4. Cloud-init rendering:
   - systemd unit and install commands included when config is present
   - fail-fast path when config is missing or invalid

Acceptance:
- Touched tests pass and phase 1 crates build.

---

### 7) Phase 1 commit plan

Commit title:
- `phase1: add guest discovery, cidata injection, and sync instanced integration`

Commit contains:
- discovery protocol (`port` field)
- guest daemon discovery + SSH relay advertisement
- sync instanced discovery client integration
- kernel cmdline control-port propagation
- CIDATA agent injection + systemd bootstrap
- targeted tests

No scope creep:
- no top-level async daemon conversion
- no tunnel async migration
- no driver async trait refactor

---

## Phase 1.1 (planned): Startup readiness hardening

Goal: avoid early shell failures while guest discovery and SSH service are still warming up.

Actions:
1. In `InstanceManager::start`, after `instanced` emits `Running`, poll control `list_services` until `ssh` is discoverable (bounded timeout + backoff).
2. In `shell-proxy`, add bounded retry around control socket connect and `open_service`.
3. Retry only transient startup errors (`instanced_unreachable`, `service_unavailable`, connection refused, timeout), fail fast on non-retryable protocol errors.
4. Return clear timeout errors that include the last observed failure reason.

Acceptance:
- `bentoctl start <name>` returns only when guest `ssh` service is discoverable or timeout is reached.
- `bentoctl shell <name>` retries transient warmup failures and succeeds once guest is ready.

---

## Phase 2 (planned): Async top-level `instanced` (Tokio multi-thread)

Goal: move control path to async runtime while preserving behavior.

Actions:
1. Introduce tokio multi-thread runtime in `instanced` main.
2. Convert Unix control socket accept loop to async listener.
3. Convert per-client control handling to async tasks.
4. Keep blocking boundaries via `spawn_blocking` where needed.
5. Add RPC timeout and cancellation handling.

Acceptance:
- Concurrent control requests are stable.
- Control protocol behavior remains unchanged.

---

## Phase 3 (planned): Async tunnel and data path migration

Goal: replace ad hoc relay threads with async I/O.

Actions:
1. Migrate tunnel relays to async bidirectional copy.
2. Migrate serial fanout to async channels or tasks.
3. Standardize half-close and shutdown semantics.
4. Add disconnect and backpressure tests.

Acceptance:
- Data path is primarily async and robust under concurrency.

---

## Phase 4 (planned): Driver boundary evolution

Goal: make driver integration async-friendly and reduce callback bridge complexity.

Actions:
1. Introduce async-facing driver boundary or adapters.
2. Isolate VZ callback bridge code in dedicated modules.
3. Normalize error taxonomy and timeout behavior.
4. Remove transitional sync shims once stable.

Acceptance:
- Async daemon-driver integration is clean and maintainable.

---

## Risks and mitigations

1. Host/guest control-port mismatch.
   - Mitigation: single internal source + kernel cmdline propagation + guest fallback.
2. Discovery RPC hangs.
   - Mitigation: explicit call timeouts and health gating.
3. Agent not installed at boot.
   - Mitigation: CIDATA fail-fast validation and cloud-init install checks.
4. Scope bleed into async refactor.
   - Mitigation: strict phase boundaries and focused commits.

---

## Tracking checklist

- [x] Phase 1 protocol finalized (`port` field)
- [x] Phase 1 guest discovery server complete
- [x] Phase 1 instanced discovery integration complete
- [x] Phase 1 VZ kernel cmdline wiring complete
- [x] Phase 1 CIDATA agent injection complete
- [x] Phase 1 systemd bootstrap complete
- [x] Phase 1 tests passing
- [x] Guest migrated to `tokio-vsock` for async accept and stream handling
- [x] Shared discovery protocol moved to `crates/bento-protocol`
- [x] Guest no longer depends on `bento-runtime`
- [x] Phase 1.1 start waits for guest discovery readiness
- [x] Phase 1.1 shell-proxy retry/backoff for transient guest startup failures
- [ ] Phase 1 commit created
- [x] Phase 2 started (async top-level instanced, multi-thread runtime)
- [x] Phase 3 started (async tunnel paths)
- [ ] Phase 4 started (driver async boundary)

## Current implementation status

Completed in code:
- Moved shared discovery protocol to `crates/bento-protocol` to avoid guest depending on host runtime internals.
- Added new guest crate `crates/bento-instance-guest` with tarpc discovery server, `/proc/cmdline` control-port parsing, random SSH service port allocation, and SSH relay to `127.0.0.1:22`.
- Refactored guest vsock networking to native async `tokio-vsock` and removed the custom `AsyncVsock` adapter.
- Renamed the guest discovery implementation type from `GuestDiscoveryImpl` to `GuestAgentServer`.
- Updated `instanced` to discover guest services via tarpc in `crates/bento-runtime/src/instance_daemon.rs` and later migrated the top-level daemon flow to async.
- Appended kernel arg `bento.guest.control_port=1027` in `crates/bento-runtime/src/driver/vz/mod.rs`.
- Added global config loader in `crates/bento-runtime/src/global_config.rs` and wired CIDATA injection + cloud-init systemd setup in `crates/bento-runtime/src/cidata.rs`.
- Started phase 2 by making `InstanceDaemon::run` async and moving runtime setup to `bentoctl instanced`.
- Started phase 2 by migrating CLI<->instanced control RPCs to tarpc with control models in `crates/bento-protocol/src/control.rs`.
- Started phase 3 by introducing per-request Unix tunnel sockets for opened services and async vsock relay plumbing.

Validation completed:
- `cargo fmt`
- `cargo test -p bento-instance-guest`
- `cargo test -p bento-runtime -p bento-instance-guest`
- `cargo test -p bentoctl`
- `cargo clippy --all --benches --tests --examples --all-features` (passes with pre-existing warnings outside phase 1 scope)

Open items before closing phase 1:
- Create the phase 1 git commit.
