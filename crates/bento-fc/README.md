# bento-fc

`bento-fc` will provide BentoBox's reusable Firecracker abstraction crate.

The crate is intentionally minimal right now while the new VZ crate is being designed first. The long-term goal is a safe, ergonomic Firecracker API that hides process management and API protocol details behind typed Rust interfaces without exposing unsafe methods across the crate boundary.

## Planned scope

- Firecracker virtual machine configuration
- async lifecycle APIs
- safe serial and vsock abstractions
- host capability validation and runtime preparation helpers

## Example

```rust,no_run
use bento_fc::FirecrackerError;

fn main() -> Result<(), FirecrackerError> {
    Ok(())
}
```
