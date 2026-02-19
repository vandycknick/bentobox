# stty -isig

vfkit \
  --cpus 4 \
  --memory 4096 \
    --bootloader "linux,kernel=../target/boxos/arch/arm64/boot/Image,initrd=../target/boxos/initramfs,cmdline=root=/dev/vda rw console=hvc0" \
  --device virtio-blk,path=./arch.img \
  --device virtio-net,nat \
  --device virtio-serial,stdio

stty isig
