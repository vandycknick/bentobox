# Resources

This directory holds guest OS build inputs and related assets.

- `resources/kernels/` contains kernel configs, track metadata, and kernel build orchestration
- `resources/initramfs/` contains the minimal initramfs payload
- `resources/rootfs/` contains full root filesystem build inputs
- `resources/busybox/` contains the busybox build used by the initramfs flow

Most generated outputs are written to `target/resources/`, while VM-built kernel artifacts are exported to `target/kernels/`.
