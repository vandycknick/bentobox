# bento-agent

Guest-side agent for Bento VMs.

`bento-agent` runs inside the Linux guest and exposes Bento's guest control plane over vsock. It is responsible for agent RPC, shell proxying, guest-side forwarding, and optional guest DNS management. The host uses it to probe guest readiness and to reach guest services without modeling each guest feature as a separate pluggable daemon.

## Overview

`bento-agent` currently does four main jobs inside the guest:

- serves the guest RPC API over vsock
- proxies shell access to the guest SSH daemon on the reserved shell vsock port
- runs the guest-side forward service used by the `forward` plugin
- optionally manages guest DNS and `resolv.conf`

The control RPC port is selected from the kernel command line via `bento.guest.port`. If that kernel arg is missing or invalid, the agent falls back to Bento's default control port.

At startup the agent:

1. initializes tracing
2. loads guest config from disk
3. reads the control port from `/proc/cmdline`
4. starts DNS if enabled
5. starts the shell proxy if enabled
6. starts the forward service if enabled
7. starts the control RPC server

The active RPC surface currently includes:

- `Ping`
- `Health`
- `GetSystemInfo`

## Config

The default guest config path is:

```text
/etc/bento/agent.yaml
```

If the file is missing, the agent falls back to its built-in defaults.

Current config shape:

```yaml
ssh:
  enabled: true

dns:
  enabled: false
  listen_address: 127.0.0.1
  upstream_servers: []
  zones: []

forward:
  enabled: false
  port: 0
  uds: []
```

Example with all supported sections populated:

```yaml
ssh:
  enabled: true

dns:
  enabled: true
  listen_address: 127.0.0.1
  upstream_servers:
    - 1.1.1.1:53
    - 8.8.8.8:53
  zones:
    - domain: docker.internal
      authoritative: false
      records:
        - name: host
          type: CNAME
          value: host.bento.internal

forward:
  enabled: true
  port: 4000
  uds:
    - guest_path: /var/run/docker.sock
```

Notes:

- `ssh.enabled` controls whether the agent exposes the reserved shell proxy over vsock.
- `dns.enabled` enables the guest DNS server and managed `resolv.conf` behavior.
- `forward.port` must be set when `forward.enabled` is true. This is the guest-side vsock port used by the host `forward` plugin endpoint.
- `forward.uds` is an allowlist of guest Unix socket paths the forward service may connect to.
- The agent does not read its control RPC port from this file. That comes from the kernel arg owned by the host side.

## Logging

`bento-agent` writes its runtime logs to stderr.

In the default systemd/cloud-init boot path, that means agent logs are typically captured by the service manager, for example through `journalctl -u bento-agent.service`.

The guest install/bootstrap flow is separate from the agent process and logs to:

```text
/var/log/bento-guest-install.log
```

## Bootstrap

Today Bento expects `bento-agent` to be installed and started via cloud-init plus systemd.

The bootstrap path writes the agent binary and config into the guest, installs a `bento-agent.service` unit, and starts or restarts the service as needed.

When the process is running as PID 1, the current behavior is intentionally minimal. The agent detects PID 1 and logs that init mode is not implemented yet. Direct PID 1 initialization support is planned, but is still to be implemented.

That future mode is expected to cover the pieces currently delegated to the guest OS boot flow, such as early system setup and service supervision.

## Cross-Compilation

The current repo-level helper is:

```bash
make build-guest-agent
```

That target builds the guest agent binary and writes the resulting path into Bento's global config:

```text
~/.config/bento/config.yaml
```

Current target details:

- target triple: `aarch64-unknown-linux-musl`
- output binary: `target/aarch64-unknown-linux-musl/release/bento-agent`

The current flow still likely needs some tuning, especially around local toolchain assumptions and Linux-target verification on non-Linux hosts.

If you want to run the command manually, it is currently equivalent to:

```bash
cargo zigbuild -p bento-agent --target aarch64-unknown-linux-musl --release
```

## Status

This crate is Linux-guest-only. Host-side validation can be done from macOS, but full agent compilation and runtime verification still depend on having the Linux target toolchain available.
