- [ArchBoot](https://archboot.com/#releases)

# What to run

Create an image:

```sh
truncate -s 20G images/archlinux.img
```

Boot kernel with ArchBoot:

```sh
stty -isig
vfkit \
  --cpus 4 \
  --memory 4096 \
    --bootloader "linux,kernel=./images/Image,initrd=./images/initrd-aarch64.img,cmdline=root=/dev/vda rw console=hvc0" \
  --device virtio-blk,path=./images/archlinux.img \
  --device virtio-net,nat \
  --device virtio-serial,stdio
stty isig
```

# Inside the VM run the following commands:

```sh
rm -f /var/lib/pacman/sync/*.db
pacman -Syy

mkfs.btrfs -f /dev/vda
mount /dev/vda /mnt

btrfs subvolume create /mnt/@
btrfs subvolume create /mnt/@home
umount /mnt

mount -o subvol=@ /dev/vda /mnt
mkdir /mnt/home
mount -o subvol=@home /dev/vda /mnt/home

pacman -Syy arch-install-scripts
pacstrap /mnt base systemd btrfs-progs cloud-init ca-certificates ca-certificates-utils openssl openssh sudo socat vim

genfstab -U /mnt >> /mnt/etc/fstab
```

Chroot inside Linux VM.

```sh
arch-chroot /mnt

systemctl enable systemd-networkd.service
systemctl enable systemd-resolved.service
systemctl enable systemd-timesyncd.service
systemctl enable sshd.service
systemctl enable cloud-init-local.service cloud-init-network.service cloud-init-main.service

sudo rm -f /etc/resolv.conf
sudo ln -s /run/systemd/resolve/stub-resolv.conf /etc/resolv.conf

passwd -d root

cat > /etc/cloud/cloud.cfg.d/99-datasource.cfg  <<'EOF'
datasource_list: [ NoCloud, None ]
EOF

echo archlinux > /etc/hostname

truncate -s 0 /etc/machine-id
rm -f /var/lib/dbus/machine-id

passwd
exit
```

# Add VSOCK to SSH bridge service (inside chroot)

Create a systemd unit that forwards guest VSOCK port `2222` to local SSH (`127.0.0.1:22`):

```sh
cat > /etc/systemd/system/vsock-ssh-bridge.service <<'EOF'
[Unit]
Description=Bento VSOCK to SSH bridge
After=sshd.service
Requires=sshd.service

[Service]
Type=simple
ExecStart=/usr/bin/socat VSOCK-LISTEN:2222,fork,reuseaddr TCP:127.0.0.1:22
Restart=always
RestartSec=1
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true

[Install]
WantedBy=multi-user.target
EOF

systemctl enable vsock-ssh-bridge.service
```

This config keeps `sshd` reachable only on loopback and allows passwordless root login for local development.

Shrink disk

```sh
btrfs filesystem usage /mnt
```

Cleanup

```sh
umount -R /mnt
```

Package

```sh
cargo run -- images pack archlinux:202602 --image ./builders/images/archlinux.img --out ./builders/images/arch.iso.tar --os linux --arch arm64
```

# DNS management on Linux distros

## TL;DR for this image

This image uses `systemd-networkd` + `systemd-resolved`. The recommended setup is:

```sh
ln -sf /run/systemd/resolve/stub-resolv.conf /etc/resolv.conf
```

That makes apps that read `/etc/resolv.conf` talk to the local `systemd-resolved` stub (`127.0.0.53`), while `systemd-resolved` manages upstream DNS from DHCP/static config.

## How common Linux distros handle DNS

- **Arch Linux**
    - Common modern setup is `systemd-resolved` in stub mode.
    - `/etc/resolv.conf` should be a symlink to `/run/systemd/resolve/stub-resolv.conf`.
    - If `/etc/resolv.conf` is a regular file, `resolvectl status` shows `resolv.conf mode: foreign`, and name resolution can break for tools that read `resolv.conf` directly.

- **Fedora**
    - `systemd-resolved` is enabled by default in modern releases.
    - `/etc/resolv.conf` is typically managed as a stub symlink.
    - DNS routing (including split DNS with VPNs) is handled by `systemd-resolved`.

- **Ubuntu**
    - Also uses `systemd-resolved` by default in modern releases.
    - Common/default behavior is `/etc/resolv.conf` pointing at a `systemd-resolved`-managed resolver file.
    - Stub-resolver pattern is the standard path for compatibility with apps that parse `resolv.conf`.

- **Debian**
    - More mixed depending on install profile and admin choice.
    - Can run classic static `/etc/resolv.conf`, `resolvconf`, or `systemd-resolved`.
    - If `systemd-resolved` is used, symlinked resolver files are the expected pattern.

## Practical note for image builds

In some install/chroot flows, `/etc/resolv.conf` can be bind-mounted from the host, so creating the symlink from inside chroot may fail. If that happens, create the symlink from outside chroot against the target root (for example `/mnt/etc/resolv.conf`).
