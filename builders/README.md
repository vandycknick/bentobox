- [ArchBoot](https://archboot.com/#releases)

# What to run

Create an image:

```sh
truncate -s 20G archlinux.img
```

Boot kernel with ArchBoot:

```sh
stty -isig
vfkit \
  --cpus 4 \
  --memory 4096 \
    --bootloader "linux,kernel=./images/Image,initrd=./images/initrd-aarch64.img,cmdline=root=/dev/vda rw console=hvc0" \
  --device virtio-blk,path=./images/arch.img \
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
pacstrap /mnt base systemd btrfs-progs cloud-init openssh sudo

genfstab -U /mnt >> /mnt/etc/fstab
```

Chroot inside Linux VM.

```sh
arch-chroot /mnt

systemctl enable systemd-networkd
systemctl enable systemd-resolved
systemctl enable sshd
systemctl enable cloud-init

echo archlinux > /etc/hostname

passwd
exit
```

Shrink disk

```sh
btrfs filesystem usage /mnt


```

Cleanup

```sh
umount -R /mnt
```

# TODO

- [ ] Fix kernel build args, it still missing some settings that allows systemd to boot.
- [ ] Fix arch build process, I can just use pacstrap and btrfs, I just need to add some smarts to my initramfs
