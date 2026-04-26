# bento-fc

`bento-fc` is BentoBox's async Firecracker client crate.

The crate currently exposes:

- a vendored Firecracker API spec
- a Progenitor-generated low-level API client built from a checked-in OpenAPI document
- a handwritten `FirecrackerProcessBuilder`, `VirtualMachineBuilder`, and `VirtualMachine` facade
- async-friendly serial and vsock wrappers for host-side guest communication

`bento-fc` is intended to grow into the generic Firecracker integration layer for BentoBox. The first phase focuses on getting the typed API surface correct before process management, VM builders, and other ergonomic SDK pieces are layered on top.

## Scope

Current scope focuses on the API foundation:

- vendored Firecracker Swagger and OpenAPI specs
- generated Rust API code via `build.rs`
- direct Firecracker process spawning
- async-first builder and VM facade
- Firecracker-specific vsock convenience helpers

Planned follow-up scope includes:

- jailer support
- async lifecycle helpers
- higher-level VM configuration helpers

## Requirements

- Rust toolchain
- Linux host for actual Firecracker usage
- Firecracker API spec pinned under `spec/`

Normal crate builds do not require Node.js or Swagger conversion tooling.

## Example

```rust,no_run
use std::num::NonZeroU64;
use tokio::io::AsyncWriteExt;

use bento_fc::{
    types::{
        BootSource, Drive, DriveCacheType, DriveIoEngine, MachineConfiguration, Vsock,
    },
    FirecrackerProcessBuilder,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let process = FirecrackerProcessBuilder::new("firecracker", "/tmp/firecracker.socket")
        .id("demo-vm")
        .spawn()
        .await?;

    let vm = process
        .builder()
        .boot_source(BootSource {
            boot_args: Some("console=ttyS0 reboot=k panic=1".to_string()),
            initrd_path: Some("/path/to/initramfs".to_string()),
            kernel_image_path: "/path/to/vmlinux".to_string(),
        })
        .machine_config(MachineConfiguration {
            cpu_template: None,
            huge_pages: None,
            mem_size_mib: 512,
            smt: false,
            track_dirty_pages: false,
            vcpu_count: NonZeroU64::new(2).expect("non-zero vCPU count"),
        })
        .add_drive(Drive {
            cache_type: DriveCacheType::Unsafe,
            drive_id: "rootfs".to_string(),
            io_engine: DriveIoEngine::Sync,
            is_read_only: Some(false),
            is_root_device: true,
            partuuid: None,
            path_on_host: Some("/path/to/rootfs.ext4".to_string()),
            rate_limiter: None,
            socket: None,
        })
        .set_vsock(Vsock {
            guest_cid: 3,
            uds_path: "/tmp/firecracker.vsock".to_string(),
            vsock_id: None,
        })
        .start()
        .await?;

    let mut serial = process.serial()?;
    serial.write_all(b"hello serial\n").await?;

    let vsock = vm.vsock()?;
    let mut stream = vsock.connect(52).await?;
    std::io::Write::write_all(&mut stream, b"hello from host\n")?;
    Ok(())
}
```

The generated API still lives under `bento_fc::api`, but the primary crate surface now goes through the handwritten process builder, VM builder, VM handle, and guest communication helpers.

## Regenerating the API

```bash
python3 virt/bento-fc/spec/convert_openapi.py
cargo build -p bento-fc
```

The OpenAPI document is checked into `spec/`, and `build.rs` runs Progenitor from that pinned file. Normal builds do not need Swagger conversion tooling.
