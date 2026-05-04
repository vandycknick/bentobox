# bento-krun

`bento-krun` is BentoBox's libkrun integration crate and helper binary.

The crate exposes:

- a process-backed `VirtualMachineBuilder`
- a `VirtualMachine` handle for lifecycle management
- a `SerialConnection` wrapper for helper stdio access
- typed disk, mount, and vsock configuration structs

The `krun` binary is intentionally small. It parses BentoBox's flat helper arguments, configures libkrun directly, and then enters the VM. It does not use the library builder and does not expose subcommands.

## Boundary

The library and binary have different jobs, and keeping that split intact prevents callers like `vmmon` from linking against libkrun directly.

Library responsibilities:

- hold typed configuration structs
- perform structural config validation only, such as non-zero CPU and memory values
- build the flat helper command line
- spawn and manage the `krun` helper process
- set up PTY-backed stdio when `stdio_console(true)` is requested
- expose process lifecycle and serial ownership handles

Binary responsibilities:

- parse flat helper arguments
- call `bento-krun-sys` and libkrun APIs
- check libkrun feature availability immediately before calling a feature-specific libkrun API
- return contextual errors for unsupported libkrun capabilities
- enter the VM

Do not import `bento_krun_sys` from library modules. Direct libkrun access belongs in `src/bin/krun.rs` or the lower-level `bento-krun-sys` crate. The library is a launcher/wrapper around the helper binary, not a libkrun API facade.

## Scope

Current scope focuses on the libkrun path used by BentoBox today:

- direct kernel and initramfs boot
- raw block devices
- virtiofs mounts
- vsock ports backed by Unix sockets
- stdio console output
- process-backed VM lifecycle management from Rust callers

Planned follow-up scope includes:

- richer `VirtualMachine` lifecycle state
- graceful shutdown when libkrun exposes a host-side shutdown path we can rely on
- higher-level serial and vsock convenience helpers

## Requirements

- Rust toolchain
- libkrun and its runtime dependencies available at link and run time for the `krun` helper binary
- Linux or macOS host support matching the linked libkrun build

## Feature Validation

Every Bento-exposed libkrun feature that requires a capability check must be checked inside the `krun` helper binary immediately before the feature-specific libkrun API is called.

Those checks return contextual helper errors that vmmon can capture and log from the helper process. For example, attempting to attach block devices with a libkrun build that lacks `BLK` support returns an error shaped like:

```text
unsupported libkrun feature: block devices (--disk) requires libkrun feature BLK; rebuild or install a libkrun with BLK support
```

This keeps vmmon logs useful and avoids raw negative libkrun return codes when the real issue is an unsupported feature.

Feature checks must not live in the library. If they do, `vmmon` and other library users can acquire a runtime dependency on `libkrun.so` just by linking the launcher crate, which defeats the helper-process boundary.

## Example

```rust,no_run
use std::io::{Read, Write};

use bento_krun::{Disk, VirtualMachineBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut vm = VirtualMachineBuilder::new("/usr/local/bin/krun")
        .cpus(2)
        .memory_mib(1024)
        .kernel("/path/to/kernel")
        .initramfs("/path/to/initramfs")
        .cmdline(vec!["console=hvc0".to_string(), "panic=1".to_string()])
        .disk(Disk {
            block_id: "root".to_string(),
            path: "/path/to/rootfs.img".into(),
            read_only: false,
        })
        .stdio_console(true)
        .start()?;

    let mut serial = vm.serial()?;
    serial.write_all(b"hello serial\n")?;

    let mut buffer = [0; 1024];
    let _ = serial.read(&mut buffer)?;

    vm.shutdown()?;
    Ok(())
}
```

The public crate API is process-backed because BentoBox uses the `krun` helper as the libkrun execution boundary. The helper binary remains single-purpose and direct-to-libkrun, while Rust callers use the builder and VM handle facade without linking libkrun themselves.
