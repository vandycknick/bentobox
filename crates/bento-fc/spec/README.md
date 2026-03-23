# Firecracker API Specs

This directory vendors the Firecracker API description used by `bento-fc`.

- `firecracker-swagger.yaml` is the upstream Swagger 2.0 document from the Firecracker repository.
- `firecracker-openapi.yaml` is the checked-in OpenAPI 3.0 conversion used for Rust client generation.

`bento-fc` does not convert the spec during normal builds. That keeps the crate free from Node.js or other extra conversion tooling at compile time.

## Refreshing the specs

1. Replace `firecracker-swagger.yaml` with the upstream version you want to pin.
2. Run `python3 crates/bento-fc/spec/convert_openapi.py`.
3. Run `cargo build -p bento-fc`.

## Notes

- The conversion script is intentionally scoped to Firecracker's current Swagger document shape.
- `build.rs` removes `default` responses before feeding the document into Progenitor because those responses currently produce incompatible generated client output for this API.
