# BentoBox 🍱

WORK IN PROGRESS

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
