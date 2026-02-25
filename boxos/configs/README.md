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

## 9) RTC framework for direct-kernel, non-EFI boots

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

## 10) AUTOFS support for no-modules kernels (optional cleanup)

### Required config changes

- `CONFIG_AUTOFS_FS=y`

### What this enables

- Built-in autofs support.

### Why this is needed

- Removes `Failed to find module 'autofs4'` warnings when using a kernel with `CONFIG_MODULES=n`.
