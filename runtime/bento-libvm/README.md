# bento-libvm

`bento-libvm` is the Rust library boundary for managing Bento virtual machines.
It gives callers a `Runtime` entry point, then returns `Machine` handles for
lifecycle operations.

Use it when you need to create, resolve, inspect, start, stop, or remove Bento
VMs from Rust code. The crate keeps database rows, runtime state files, and
process details behind the API boundary.

```rust
use bento_libvm::{MachineRef, Runtime};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), bento_libvm::LibVmError> {
    let runtime = Runtime::from_env().await?;
    let machine = runtime.get_machine(&MachineRef::parse("devbox")?).await?;

    let data = machine.inspect().await?;
    println!("{} is {:?}", data.name, data.status);

    if !data.is_running() {
        machine.start().await?;
    }

    Ok(())
}
```

The main shapes are:

- `Runtime`, the service entry point.
- `Machine`, an operable handle for one VM.
- `MachineCreate` and `MachineUpdate`, request DTOs for caller input.
- `MachineInspectData`, an owned snapshot returned by inspect and mutation calls.

See the generated Rust docs for the full method and field-level API.
