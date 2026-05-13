# BentoBox Terminology

This document defines the virtualization terms used in BentoBox. The terms intentionally line up with the KVM, Firecracker, crosvm, Cloud Hypervisor, Virtualization.framework, and libvirt ecosystems where that makes the code easier to reason about.

## Virtual Machine / VM

A virtual machine is the guest environment managed by BentoBox.

In user-facing documentation, use "VM" when referring to an instance created, started, stopped, or deleted by BentoBox.

Examples:

- `bentoctl create dev`
- `bentoctl start dev`
- `bentoctl stop dev`

## microVM

A microVM is a lightweight VM optimized for fast startup, low overhead, and a small virtual device model.

BentoBox primarily targets microVM-style workloads. Not every supported host virtualization stack needs to use the exact term "microVM", but BentoBox should prefer implementations and configurations that fit this model.

## VMM

A virtual machine monitor is the low-level virtualization implementation that runs a guest VM and provides its virtual device model.

Examples:

- Firecracker
- crosvm
- Cloud Hypervisor
- QEMU
- VZ-backed virtualization on macOS, where applicable

BentoBox's abstraction crate is not a VMM. In BentoBox, VMM refers to a concrete implementation underneath `bento-virt`.

## Hypervisor

A hypervisor is the host virtualization layer or full virtualization stack that makes guest execution possible.

Depending on context this may refer to KVM, Apple's virtualization stack, Hypervisor.framework, or the broader host virtualization system.

BentoBox itself is not a hypervisor.

## KVM

KVM is the Linux kernel virtualization interface exposed through `/dev/kvm`.

KVM provides kernel support for creating and running virtual machines through ioctls on KVM-related file descriptors. It does not provide BentoBox's lifecycle, configuration, networking, storage, or monitoring model.

## Virtualization Backend

A virtualization backend is the concrete host implementation BentoBox uses to run a VM.

Current BentoBox runtime backend selection is internal and host-driven:

- macOS uses Apple Virtualization.framework through `bento-vz`
- Linux uses libkrun/krun through `bento-krun`

Users do not select a backend in `VmSpec`. `VmSpec` describes the VM, while BentoBox chooses the host implementation at compile time.

## Backend Driver

A backend driver is the Rust adapter code that implements support for a virtualization backend inside BentoBox.

Examples:

- `bento-vz`
- `bento-krun`

Use "driver" for BentoBox adapter code. Use "VMM" for the underlying virtualization implementation.

## `bento-virt`

`bento-virt` is BentoBox's host virtualization facade.

It exposes the common Rust API that `bento-vmmon` uses to create, start, stop, and communicate with a VM. The concrete implementation is selected at compile time by host platform.

`bento-virt` is not a VMM or a hypervisor.

## `bento-vmmon`

`bento-vmmon` is the VM monitor process.

It supervises one running VM, exposes monitor and control APIs, tracks lifecycle state, handles guest readiness, and participates in cleanup and reconciliation.

`bento-vmmon` uses `bento-virt` to start and control the host-selected virtualization implementation.

## `bento-libvm`

`bento-libvm` is the higher-level VM orchestration library.

It owns product-level lifecycle semantics, persisted state, image handling, launch flow, and interaction with `bento-vmmon`.

Its role is similar in spirit to how libpod sits above lower-level container runtime pieces.

## Guest Agent

The guest agent is software running inside the guest VM.

It is separate from the VMM, `bento-virt`, and `bento-vmmon`. It provides guest-side services such as readiness, shell support, or bootstrap integration.

## Host

The host is the operating system running BentoBox.

Examples:

- macOS host using Apple Virtualization.framework
- Linux host using KVM-backed virtualization

## Guest

The guest is the operating system running inside the VM.
