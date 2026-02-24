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
pacstrap /mnt base systemd btrfs-progs cloud-init openssh sudo socat vim

genfstab -U /mnt >> /mnt/etc/fstab
```

Chroot inside Linux VM.

```sh
arch-chroot /mnt

systemctl enable systemd-networkd.service
systemctl enable systemd-resolved.service
systemctl enable sshd.service
systemctl enable cloud-init-local.service cloud-init-network.service cloud-init-main.service

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
