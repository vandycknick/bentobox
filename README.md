# BentoBox 🍱

BentoBox is a microVM manager that boots a full Linux environment in seconds. It is highly configurable, so you can tune nearly every aspect of the system. Whether you want a WSL-like development environment on macOS, a fresh Docker Desktop alternative, or a secure sandbox for agentic workflows, BentoBox has you covered. Run it on your laptop, on servers, in the cloud, or wherever you need it.

## Runtime Backends

- macOS: Apple `Virtualization.framework`
- Linux: Firecracker backend (**work in progress**)

## Inspiration

BentoBox draws inspiration from these projects, which helped shape its architecture and developer experience:

- [macosvm](https://github.com/s-u/macosvm)
- [UTM](https://github.com/utmapp/UTM)
- [Lima](https://github.com/lima-vm/lima)
- [vfkit](https://github.com/crc-org/vfkit)

## Getting Started

Install with Nix profile:

```bash
nix profile install .#bentoctl
```

Or build locally with Nix:

```bash
nix build .#bentoctl
./result/bin/bentoctl --help
```

## Usage

```text
Bentobox instance lifecycle control

Usage: bentoctl [OPTIONS] <COMMAND>

Commands:
  create
  start
  stop
  shell
  delete
  list
  status
  instanced
  images

Options:
  -v, --verbose...
  -h, --help        Print help
```

## Quick VM Lifecycle

Create a VM from an image:

```bash
bentoctl create dev --image <name-or-oci-ref>
```

Start it:

```bash
bentoctl start dev
```

Open a shell:

```bash
bentoctl shell dev
```

Stop it:

```bash
bentoctl stop dev
```

List instances:

```bash
bentoctl list
```
