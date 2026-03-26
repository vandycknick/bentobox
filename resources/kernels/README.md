# Kernel Resources

This directory owns the guest kernel build inputs for Bento.

## Supported tracks

- `stable`: `6.19.7`
- `longterm`: `6.18.17`
- `longterm5`: `5.15.202`

Build with:

```bash
bentoctl exec arch -- make kernel TRACK=stable ARCH=arm64
bentoctl exec arch -- make kernel TRACK=longterm ARCH=arm64
bentoctl exec arch -- make kernel TRACK=longterm5 ARCH=arm64
```

Kernel source, build, and cache state live inside the guest under `$HOME/.cache/bento/kernels/`.

Final exported artifacts land in the mounted repo under `target/kernels/<track>-<arch>-<version>/`.

The canonical arm64 config baseline lives at `resources/kernels/configs/arm64-base.config`. Track-specific config drift lives in `resources/kernels/configs/overlays/<track>.config`, which gets appended before `olddefconfig` runs. The `manifest.toml` file records the current manually pinned track versions.

# Kernel config changes from the VM bring-up session

This file tracks kernel-side config changes identified while debugging package updates, DNS/TLS time issues, and VM boot behavior.

## 1) Core VM runtime filesystem and device node support

### Required config changes

- `CONFIG_MEMFD_CREATE=y`
- `CONFIG_DEVTMPFS=y`
- `CONFIG_DEVTMPFS_MOUNT=y`
- `CONFIG_STANDALONE=y`
- `CONFIG_PREVENT_FIRMWARE_BUILD=y`
- `CONFIG_TMPFS=y`
- `CONFIG_TMPFS_POSIX_ACL=y`
- `CONFIG_TMPFS_XATTR=y`
- `CONFIG_TMPFS_QUOTA=y`

### What this enables

- Anonymous in-memory file descriptors via `memfd`.
- Automatic population of `/dev` via devtmpfs.
- Tmpfs-backed runtime filesystems with ACL/xattr/quota support.

### Why this is needed

- Supports modern userspace and service manager expectations during VM boot.
- Avoids missing device-node and runtime-filesystem issues in minimal images.

## 2) PCI host controller support for virtualized device enumeration

### Required config changes

- `CONFIG_PCI_ECAM=y`
- `CONFIG_PCI_HOST_COMMON=y`
- `CONFIG_PCI_HOST_GENERIC=y`

### What this enables

- Generic ECAM-based PCI configuration space access.
- Generic PCI host bridge initialization on arm64 virtual platforms.
- PCI bus discovery and enumeration for virtual devices presented by the hypervisor.

### Why this is needed

- In Linux guests on Apple `Virtualization.framework`, many paravirtualized devices are exposed through a PCI topology.
- The guest kernel must initialize the virtual PCI host bridge and scan buses, otherwise devices can fail to appear even when their functional drivers are enabled.
- Enabling the ECAM and generic host-controller path provides a portable baseline for VM device discovery across common `Virtualization.framework` Linux guest configurations.

## 3) Cgroup v2 and file event support for modern userspace

### Required config changes

- `CONFIG_CGROUPS=y`
- `CONFIG_INOTIFY_USER=y`

### What this enables

- Unified cgroup v2 hierarchy mounting and management.
- Inotify-based user-space file event watching.

### Why this is needed

- Required by modern init and runtime tooling that expects cgroup support and filesystem notifications.
- `inotify` is the standard Linux userspace API for file and directory change events, many daemons and developer tools rely on it instead of polling.
- Without `CONFIG_INOTIFY_USER`, programs using `inotify_init(2)` and `inotify_init1(2)` fail, which causes silent feature loss or hard failures in minimal VM images.

## 4) ISO cloud-init media and character set support

### Required config changes

- `CONFIG_ISO9660_FS=y`
- `CONFIG_JOLIET=y`
- `CONFIG_ZISOFS=y`
- `CONFIG_NLS=y`
- `CONFIG_NLS_DEFAULT="y"`

### What this enables

- Mounting ISO9660 cloud-init seed media.
- Joliet filename extensions and zisofs compressed ISO support.
- Kernel NLS framework for filesystems and features requiring charset handling.

### Why this is needed

- Enables reading `cidata` ISO images used for cloud-init provisioning in VM workflows.

## 5) Disable IPv6 SIT tunneling

### Required config changes

- `# CONFIG_IPV6_SIT is not set`

### What this enables

- Removes SIT tunnel support from the kernel.

### Why this is needed

- Eliminates unused tunnel capability and resolves the prior SIT-related issue in this VM kernel profile.

## 6) Pacman sandbox support (Landlock)

### Required config changes

- `CONFIG_SECURITYFS=y`
- `CONFIG_SECURITY_LANDLOCK=y`
- Keep `CONFIG_SECURITY=y`
- Ensure `landlock` is present in `CONFIG_LSM`

### What this enables

- Landlock-based filesystem sandboxing used by pacman/libalpm.
- General LSM support for userspace sandboxing workflows.

### Why this is needed

- Landlock can be used to create a sandbox around agent processes.
- Without Landlock support, pacman fails with errors such as:
    - `restricting filesystem access failed because Landlock is not supported by the kernel`

## 7) Seccomp support for sandbox compatibility

### Required config changes

- `CONFIG_SECCOMP=y`

### What this enables

- Syscall filtering used by modern sandboxed user-space.

### Why this is needed

- Improves compatibility with hardened and sandboxed execution paths, including package management and service sandboxes.

## 8) Lockdown LSM defaults (optional hardening baseline)

### Required config changes

- `CONFIG_SECURITY_LOCKDOWN_LSM=y`
- `CONFIG_SECURITY_LOCKDOWN_LSM_EARLY=n`
- Default lockdown mode: `LOCK_DOWN_KERNEL_FORCE_NONE`

### What this enables

- Lockdown framework is available without forcing restrictive behavior at boot.

### Why this is needed

- Keeps hardening hooks available while avoiding breakage in development VM workflows.

## 9) Nested virtualization support for arm64 guest kernels

### Required config changes

- `CONFIG_KVM=y`
- `CONFIG_VHOST_MENU=y`
- `CONFIG_VHOST_VSOCK=y`
- `CONFIG_TUN=y`
- `CONFIG_VHOST_NET=y`

### What this enables

- In-guest KVM host support on arm64 so the guest can act as an L1 hypervisor.
- Vhost-backed vsock and virtio-net acceleration paths used by nested virtualization stacks.
- TUN support for common nested guest networking setups.

### Why this is needed

- Nested virtualization needs the guest kernel to expose `/dev/kvm` and the supporting virtualization datapath, not just guest virtio drivers.
- `VHOST_VSOCK` matches Bento's current vsock-heavy transport model.
- `TUN` and `VHOST_NET` make nested guest networking usable instead of deeply annoying.

## 10) Kernel config introspection (observability)

### Required config changes

- `CONFIG_IKCONFIG=y`
- `CONFIG_IKCONFIG_PROC=y`

### What this enables

- Embedding the kernel config into the built kernel.
- Reading the running kernel config from `/proc/config.gz`.

### Why this is needed

- Not required for virtualization itself.
- Makes it easy to verify a booted kernel really contains the expected KVM and vhost flags without playing guess-the-image.

## 11) RTC framework for direct-kernel, non-EFI boots

### Required config changes

- `CONFIG_RTC_CLASS=y`
- `CONFIG_RTC_HCTOSYS=y`
- `CONFIG_RTC_SYSTOHC=y`
- `CONFIG_RTC_HCTOSYS_DEVICE="rtc0"`

### What this enables

- Linux RTC subsystem support and RTC-to-system-clock integration when a compatible virtual RTC device is present.

### Why this is needed

- Current kernel has RTC core disabled, causing `RTC time: n/a`.
- This does not guarantee RTC in direct kernel boot mode if the hypervisor path does not expose a compatible RTC device.

## 12) AUTOFS support for no-modules kernels (optional cleanup)

### Required config changes

- `CONFIG_AUTOFS_FS=y`

### What this enables

- Built-in autofs support.

### Why this is needed

- Removes `Failed to find module 'autofs4'` warnings when using a kernel with `CONFIG_MODULES=n`.

## 13) Rootful Docker guest support

### Required config changes

- Core container isolation and policy:
    - `CONFIG_NAMESPACES=y`
    - `CONFIG_UTS_NS=y`
    - `CONFIG_IPC_NS=y`
    - `CONFIG_PID_NS=y`
    - `CONFIG_NET_NS=y`
    - `CONFIG_USER_NS=y`
    - `CONFIG_CGROUPS=y`
    - `CONFIG_MEMCG=y`
    - `CONFIG_BLK_CGROUP=y`
    - `CONFIG_CGROUP_PIDS=y`
    - `CONFIG_CGROUP_DEVICE=y`
    - `CONFIG_CPUSETS=y`
    - `CONFIG_CGROUP_CPUACCT=y`
    - `CONFIG_SECCOMP=y`
    - `CONFIG_SECCOMP_FILTER=y`
- Docker bridge networking and packet path:
    - `CONFIG_BRIDGE=y`
    - `CONFIG_BRIDGE_NETFILTER=y`
    - `CONFIG_VETH=y`
    - `CONFIG_INET=y`
    - `CONFIG_IPV6=y`
    - `CONFIG_NETFILTER=y`
    - `CONFIG_NF_CONNTRACK=y`
    - `CONFIG_NETFILTER_XTABLES=y`
    - `CONFIG_NETFILTER_XTABLES_LEGACY=y`
    - `CONFIG_NETFILTER_XTABLES_COMPAT=y`
    - `CONFIG_NETFILTER_XT_MATCH_ADDRTYPE=y`
    - `CONFIG_NETFILTER_XT_MATCH_CONNTRACK=y`
    - `CONFIG_NETFILTER_XT_NAT=y`
    - `CONFIG_NETFILTER_XT_TARGET_MASQUERADE=y`
- Legacy `iptables` and `ip6tables` tables used by current rootful Docker userspace:
    - `CONFIG_IP_NF_IPTABLES=y`
    - `CONFIG_IP_NF_FILTER=y`
    - `CONFIG_IP_NF_MANGLE=y`
    - `CONFIG_IP_NF_NAT=y`
    - `CONFIG_IP_NF_RAW=y`
    - `CONFIG_IP_NF_TARGET_MASQUERADE=y`
    - `CONFIG_IP6_NF_IPTABLES=y`
    - `CONFIG_IP6_NF_FILTER=y`
    - `CONFIG_IP6_NF_MANGLE=y`
    - `CONFIG_IP6_NF_NAT=y`
    - `CONFIG_IP6_NF_RAW=y`
    - `CONFIG_IP6_NF_TARGET_MASQUERADE=y`
- Storage and runtime basics:
    - `CONFIG_OVERLAY_FS=y`
    - `CONFIG_UNIX=y`
    - `CONFIG_PACKET=y`
    - `CONFIG_POSIX_MQUEUE=y`

### What this enables

- Namespace isolation, cgroup accounting, and seccomp filtering for ordinary container startup.
- Legacy IPv4 and IPv6 `iptables` table support used by current rootful Docker startup paths, including the `raw` table.
- The legacy xtables kernel path required by current `iptables-legacy` and `ip6tables-legacy` userspace on newer kernels.
- Connection-tracking matches used by Docker bridge firewall rules such as `-m conntrack --ctstate RELATED,ESTABLISHED`.
- Compatibility support for `iptables-legacy` and `ip6tables-legacy` userspace against the kernel xtables path.
- Docker bridge networking, including `docker0` and veth peer creation for containers.
- Overlay filesystem support for the `overlay2` storage driver.

### Why this is needed

- Fixes Docker daemon startup failures such as:
    - `iptables ... can't initialize iptables table 'nat': Table does not exist`
    - `iptables ... can't initialize iptables table 'raw': Table does not exist`
    - `ip6tables ... can't initialize ip6tables table 'nat'` or `filter`
    - `Extension conntrack revision 0 not supported, missing kernel module?`
- Prevents `olddefconfig` on newer kernels from silently dropping legacy `IP*_NF_*` and `IP6*_NF_*` table support when `CONFIG_NETFILTER_XTABLES_LEGACY=y` is missing.
- Lets Docker create the `DOCKER` NAT chain and MASQUERADE rules when using `iptables-legacy`.
- Lets Docker install direct access filtering rules in `raw/PREROUTING`, which it uses to drop non-bridge traffic headed at container addresses.
- Lets Docker install IPv6 chains when ip6tables support is enabled in userspace.
- Provides the baseline guest kernel networking and storage features needed for Bento's current rootful Docker extension model.

### Notes

- This repo's arm guest kernel profile currently disables modules, so these options must be built in, not left as modules.
- `CONFIG_NF_TABLES=y` alone is not enough for the current Docker setup, because the guest userspace is using `iptables-legacy` rather than an nft-only path.
- On newer kernels, `CONFIG_NETFILTER_XTABLES_LEGACY=y` is required alongside the legacy `IP*_NF_*` and `IP6*_NF_*` symbols or Docker can still fail with missing `nat`/`raw` tables.
- `CONFIG_IP_NF_RAW=y` and `CONFIG_IP6_NF_RAW=y` are easy to miss because Docker often fails later on the first visible `raw` table access rather than during its initial capability checks.
- `CONFIG_NETFILTER_XTABLES_COMPAT=y` is part of that legacy userspace path, and missing it can surface as conntrack match failures or legacy table initialization failures even when the newer `IP*_NF_*` options are enabled.
- `CONFIG_NETFILTER_XTABLES_LEGACY` depends on `!PREEMPT_RT` on newer kernels, so a PREEMPT_RT kernel and the current Docker `iptables-legacy` path are not friends.

## 14) Firecracker arm64 serial console support

### Required config changes

- `CONFIG_PRINTK=y`
- `CONFIG_SERIAL_8250=y`
- `CONFIG_SERIAL_8250_CONSOLE=y`
- `CONFIG_SERIAL_CORE=y`
- `CONFIG_SERIAL_CORE_CONSOLE=y`
- `CONFIG_SERIAL_OF_PLATFORM=y`
- `CONFIG_DEVTMPFS=y`
- `CONFIG_DEVTMPFS_MOUNT=y`

### What this enables

- Kernel log output on the Firecracker serial console.
- Proper registration of the guest serial device on arm64 when it is described via Device Tree.
- Userspace access to the same serial-backed console used during early boot.
- Automatic population of `/dev`, including console device nodes needed by minimal initramfs environments.

### Why this is needed

- On arm64 Firecracker guests, the serial device is discovered through the Device Tree path rather than legacy PC-style probing.
- `CONFIG_SERIAL_OF_PLATFORM=y` is required so the kernel can bind the 8250-compatible UART exposed by Firecracker and make `ttyS0` behave correctly beyond the earliest boot messages.
- Without this option, the kernel may still emit some early console output, but `/init`, shell fallback paths, and other userspace console interaction can fail or behave inconsistently.
- `CONFIG_SERIAL_8250_CONSOLE` and `CONFIG_SERIAL_CORE_CONSOLE` provide the actual serial console plumbing, while `CONFIG_PRINTK` keeps kernel logs visible during bring-up.
- `CONFIG_DEVTMPFS` and `CONFIG_DEVTMPFS_MOUNT` ensure `/dev/console` and related device nodes exist in minimal initramfs boots without requiring a full userspace device manager.

## 15) VZ requestSTop

- `CONFIG_GPIO_PL061`
- `CONFIG_INPUT_EVDEV`
- `CONFIG_KEYBOARD_GPIO`

## What this enables

These are the important guest kernel bits for Apple Silicon requestStop() support.
Userspace side
You need something in userspace to react to the power-button event and actually call shutdown.
The wiki specifically suggests:

- acpid
  with a handler like:
  mkdir -p /etc/acpi/PWRF
  echo '#!/bin/sh' > /etc/acpi/PWRF/00000080
  echo 'poweroff' >> /etc/acpi/PWRF/00000080
  chmod +x /etc/acpi/PWRF/00000080
  acpid

### Notes

- For Bento's current Firecracker arm64 flow, `CONFIG_SERIAL_OF_PLATFORM=y` was the key missing option that made the initramfs shell and userspace console behavior start working correctly.
- The matching kernel boot args should still include `console=ttyS0`, and on arm64 Firecracker it is worth considering `keep_bootcon` during early bring-up.
- These options are not Firecracker-exclusive in a strict kernel sense, but they are part of the practical minimum for reliable Firecracker serial-console behavior on arm64.
