# Capabilities And Profiles

Bento now models guest features as capabilities, with profiles as convenience bundles that expand
into resolved capability config at startup time.

## Core Ideas

- instance configs persist profile references
- agent only sees capabilities, not profile names
- endpoints are the concrete things Bento exposes, like `ssh`, `docker.sock`, or a forwarded TCP port

## Profile Files

Profiles are YAML files loaded from `~/.config/bento/profiles/<name>.yaml`.

Repo-provided starter profiles live under `config/profiles/` and can be copied into your Bento
config directory. You can sync them with `make sync-profiles`.

## Current Capabilities

- `ssh`, enabled by default, startup-required
- `dns`, disabled by default unless a profile enables it
- `forward`, disabled by default unless a profile enables it

DNS discovers the host-facing resolver from the guest default gateway. Any configured
`upstream_servers` are appended as fallback resolvers.

## Current Profiles

### `docker`

The `docker` profile enables:

- the `forward` capability with dynamic TCP port discovery
- a configured UDS forward for `/var/run/docker.sock`
- a DNS CNAME record for `host.docker.internal`

## Bootstrap

Bootstrap is enabled automatically when needed.

Today that means Bento enables bootstrap when:

- you provide `--userdata`
- the resolved capability set requires guest bootstrap
- you enable `--rosetta`

Current behavior note: Bento rotates cloud-init `instance-id` during bootstrap rebuild, so cloud-init
reruns on each boot for bootstrap-enabled VMs.

## Creating A VM

Profiles are selected during create time.

Example:

```bash
bentoctl create dev --image <image> --profile docker
```

## One-Off Start Profiles

You can also apply profiles only for a single boot:

```bash
bentoctl start dev --profile docker
```

These extra profile references are resolved at startup and are not written back into the instance
config.

## Readiness

`bentoctl start` waits for the VM and guest agent, then waits for all startup-required capabilities.

Today that mainly affects `ssh`.

## Guest DNS

When the DNS capability is enabled, `agent` manages guest resolver configuration by writing
`/etc/bento/resolv.conf` and replacing `/etc/resolv.conf` with a symlink to that managed file.
It also injects `host.bento.internal` to point back at the discovered host gateway address. The
`docker` profile adds `host.docker.internal` as a CNAME in the `docker.internal` zone.

## Status

`bentoctl status <vm>` reports:

- VM lifecycle state
- guest lifecycle state
- capability health
- resolved endpoints

Example shape:

```text
name: dev
process: Running
vm: running
guest: running
ready: yes
capabilities:
  - ssh enabled=true startup_required=true configured=true running=true
  - dns enabled=true startup_required=false configured=true running=true
  - forward enabled=true startup_required=false configured=true running=true
endpoints:
  - ssh guest=127.0.0.1:22 host= active=true
  - docker guest=/var/run/docker.sock host=/.../sock/docker.sock active=true
```

## Notes

- `ssh` remains the default shell/exec path when enabled
- `forward` owns both dynamic TCP forwarding and configured UDS forwarding
- `serial` is still a core runtime endpoint, not a capability
