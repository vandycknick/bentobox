#!/usr/bin/env bash
set -eou pipefail

echo "Syncing pacman database."
rm -f /var/lib/pacman/sync/*.db
pacman -Syy
# pacman -Syy arch-install-scripts

if ! findmnt -rn -S /dev/vda > /dev/null; then
    echo "/dev/vda not mounted — formatting and mounting"

    mkfs.btrfs -f /dev/vda
    mount /dev/vda /mnt

    btrfs subvolume create /mnt/@
    btrfs subvolume create /mnt/@home
    umount /mnt

    mount -o subvol=@ /dev/vda /mnt
    mkdir /mnt/home
    mount -o subvol=@home /dev/vda /mnt/home
else
    echo "/dev/vda already mounted"
fi

pacstrap /mnt base systemd btrfs-progs cloud-init ca-certificates ca-certificates-utils openssl openssh sudo socat vim

genfstab -U /mnt >> /mnt/etc/fstab

echo
echo "Starting arch-chroot"
arch-chroot /mnt /bin/bash <<'CHROOT'
set -euo pipefail

systemctl enable systemd-networkd.service
systemctl enable systemd-resolved.service
systemctl enable systemd-timesyncd.service
systemctl enable sshd.service
systemctl enable cloud-init-local.service cloud-init-network.service cloud-init-main.service

ln -sf /run/systemd/resolve/stub-resolv.conf /etc/resolv.conf || true

passwd -d root

cat > /etc/cloud/cloud.cfg.d/99-datasource.cfg  <<'EOF'
datasource_list: [ NoCloud, None ]
EOF

echo archlinux > /etc/hostname

truncate -s 0 /etc/machine-id
rm -f /var/lib/dbus/machine-id

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
exit
CHROOT

umount -R /mnt
