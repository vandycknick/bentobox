# BentoBox 🍱

WORK IN PROGRESS

## Reading

- vsock: https://archive.fosdem.org/2021/schedule/event/vai_virtio_vsock/attachments/slides/4419/export/events/attachments/vai_virtio_vsock/slides/4419/FOSDEM_2021_vsock.pdf

## Inspiration

- [macosvm](https://github.com/s-u/macosvm)
- [UTM](https://github.com/utmapp/UTM)
- [Lima](https://github.com/lima-vm/lima)
- [vfkit](https://github.com/crc-org/vfkit)

## Image Management (V1)

- `bentoctl images list` (alias: `bentoctl image list`)
- `bentoctl images pull <oci-ref> [--name <alias>]`
- `bentoctl images import <path-to-oci-layout-or-archive>`
- `bentoctl images pack --image <path-to-rootfs.img> --os <os> --arch <arch> [--out <artifact.oci.tar>] <name>`
- `bentoctl create <name> --image <name-or-oci-ref>`
- `bentoctl list`

## Shell Access (VSOCK)

- `bentoctl shell <name>` opens an SSH shell through the instance daemon over VSOCK.
- `bentoctl shell <name> --user <user>` selects the SSH user (defaults to `root`).
- Guest requirement: run a VSOCK bridge service that forwards `VSOCK:2222` to `127.0.0.1:22` (see `builders/README.md`).

### Common Failures

- `instance_not_found`: instance name does not exist.
- `instance_not_running`: start the VM first with `bentoctl start <name>`.
- `instanced_unreachable`: control socket is missing, usually the instance daemon is not running.
- `guest_port_unreachable`: guest bridge is not running, or guest `sshd` is not reachable on loopback.
- ``ssh` command not found`: install OpenSSH client on the host.

### Troubleshooting

- Set `BENTO_SHELL_DEBUG=1` to print relay byte counters from `shell-proxy` and `instanced`.
- Check daemon logs in `~/.local/share/bento/<name>/id.stder.log` and serial logs in `~/.local/share/bento/<name>/serial.log`.

### Manual Disk Expansion

Grow the instance root disk file on the host:

```bash
truncate -s +10G /path/to/instance/rootfs.img
```

Then grow the filesystem inside the guest (example for ext4 on `/dev/vda1`):

```bash
sudo growpart /dev/vda 1
sudo resize2fs /dev/vda1
```
