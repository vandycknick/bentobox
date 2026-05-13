# BentoBox 🍱

BentoBox is a microVM manager that boots a full Linux environment in seconds. It is highly configurable, so you can tune nearly every aspect of the system. Whether you want a WSL-like development environment on macOS, a fresh Docker Desktop alternative, or a secure sandbox for agentic workflows, BentoBox has you covered. Run it on your laptop, on servers, in the cloud, or wherever you need it.

## Runtime Backends

- macOS: Apple `Virtualization.framework`
- Linux: libkrun through the `krun` helper

Backend selection is internal to BentoBox and depends on the host platform. `VmSpec` describes the VM; users do not choose the backend.

See [`docs/terminology.md`](docs/terminology.md) for the vocabulary BentoBox uses around VMs, VMMs, hypervisors, KVM, microVMs, and backend drivers.

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
  vmmon
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

Enable nested virtualization for supported macOS VZ hosts:

```bash
bentoctl create dev --image <name-or-oci-ref> --nested-virtualization
```

This is currently VZ-only and still depends on host macOS and hardware support.

Enable Rosetta for x86_64 Linux binaries in supported macOS VZ guests:

```bash
bentoctl create dev --image <name-or-oci-ref> --rosetta
```

This currently requires Apple silicon, macOS 13 or newer, and Rosetta to already be installed with `softwareupdate --install-rosetta`.

Enable the Bento guest agent explicitly when you want guest bootstrap artifacts to be injected:

```bash
bentoctl create dev --image <name-or-oci-ref> --agent
```

Images used with `--agent`, `--userdata`, or `--rosetta` are expected to support the current cloud-init/CIDATA bootstrap path. Bentobox assumes that image contract and does not validate it at create time.

Start it:

```bash
bentoctl start dev
```

Open a shell:

```bash
bentoctl shell dev
```

Run a single command over SSH, while best-effort `cd`-ing into your current host working directory first:

```bash
bentoctl exec dev -- pwd
```

Stop it:

```bash
bentoctl stop dev
```

List instances:

```bash
bentoctl list
```

## More Docs
