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
mkfs.btrfs -f /dev/vda
mount /dev/vda /mnt

btrfs subvolume create /mnt/@
btrfs subvolume create /mnt/@home
umount /mnt

mount -o subvol=@ /dev/vda /mnt
mkdir /mnt/home
mount -o subvol=@home /dev/vda /mnt/home

pacman -Sy arch-install-scripts
pacstrap /mnt base systemd btrfs-progs cloud-init openssh sudo socat

genfstab -U /mnt >> /mnt/etc/fstab
```

Chroot inside Linux VM.

```sh
arch-chroot /mnt

systemctl enable systemd-networkd
systemctl enable systemd-resolved
systemctl enable sshd
systemctl enable cloud-init

passwd -d root

cat > /etc/ssh/sshd_config.d/10-bento.conf <<'EOF'
ListenAddress 127.0.0.1
PasswordAuthentication yes
KbdInteractiveAuthentication no
PubkeyAuthentication no
PermitRootLogin yes
PermitEmptyPasswords yes
ChallengeResponseAuthentication no
UsePAM no
EOF

echo archlinux > /etc/hostname

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
cargo run -- images pack ghcr.io/vandycknick/bento-arch:v2.0.0 --image ./builders/images/archlinux.img --out ./builders/images/arch.iso.tar --os linux --arch arm64
```

# TODO

- [ ] Fix kernel build args, it still missing some settings that allows systemd to boot.
- [ ] Fix arch build process, I can just use pacstrap and btrfs, I just need to add some smarts to my initramfs
