# libkrun Implicit Behaviors

BentoBox treats the `krun` helper as an explicit VM launcher. A missing libkrun API call must not silently create host integration, guest devices, inherited environment, or host port exposure.

## Runtime Defaults

Every helper-created context must do the following before adding optional devices:

1. Call `krun_disable_implicit_console()`.
2. Call `krun_disable_implicit_vsock()`.
3. Call `krun_set_port_map()` with an empty array.
4. Add networking only when `--network` is not `none`.

If a console is needed, the helper adds one explicitly with `krun_add_virtio_console_default()` and sets `console=hvc0`. If vsock ports are configured, the helper adds one explicit vsock device with `krun_add_vsock(ctx, 0)`, keeping TSI hijacking disabled.

## Inventory

| Behavior | Trigger | Default libkrun behavior | BentoBox behavior | Platform notes |
| --- | --- | --- | --- | --- |
| Console device | Omit `krun_disable_implicit_console()` | Creates a console automatically | Always disabled, then explicitly added only for `--stdio-console` | Applies on Linux and macOS |
| Vsock device | Omit `krun_disable_implicit_vsock()` | Creates a vsock device automatically, with TSI selected heuristically | Always disabled, then explicitly added with TSI features `0` when vsock ports exist | Applies on Linux and macOS |
| TSI networking | Add no virtio-net device and leave implicit vsock enabled | Falls back to Transparent Socket Impersonation | Disabled by disabling implicit vsock and never enabling TSI features | Applies on Linux and macOS |
| Guest port exposure | Omit `krun_set_port_map()` or pass `NULL` | May expose all guest listening ports to the host | Always passes an explicit empty port map | Applies on Linux and macOS |
| Environment inheritance | Call `krun_set_exec()` or `krun_set_env()` with `NULL` | Inherits host process environment | Current helper does not use exec-mode APIs; future exec-mode code must pass an explicit env array | Applies on Linux and macOS |
| Unixgram networking | Call `krun_add_net_unixgram()` | Adds explicit virtio-net and prevents TSI fallback | Available via `--network unixgram` with `--net-peer` and `--net-mac` | Current BentoBox gvproxy path |
| Unixstream networking | Call `krun_add_net_unixstream()` | Adds explicit virtio-net and prevents TSI fallback | Available via `--network unixstream` with `--net-peer` and `--net-mac` | Suitable for passt/socket_vmnet-style peers |
| TAP networking | Call `krun_add_net_tap()` | Adds explicit virtio-net and prevents TSI fallback | Available via `--network tap` with `--net-tap-name` and `--net-mac` | Linux only |

## Networking Modes

`--network none` means no guest network device. It is the default and must not fall back to TSI.

`--network unixgram` connects a virtio-net device to a datagram Unix socket peer. The helper creates its local datagram socket next to the peer and passes the connected fd to libkrun.

`--network unixstream` connects a virtio-net device to a stream Unix socket path. The helper passes the path directly to libkrun.

`--network tap` connects a virtio-net device to an existing TAP interface by name. Validation rejects this mode on non-Linux hosts.

## Parent Liveness

The parent process passes the helper a watchdog pipe read fd in `BENTO_KRUN_WATCHDOG_FD` and holds the write fd for the VM lifetime. If the parent dies, the write fd closes, the helper observes `POLLHUP`, and exits. This avoids orphaned helper processes without relying on Linux-only `PR_SET_PDEATHSIG`.
