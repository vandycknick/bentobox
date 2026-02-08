KERNEL_CHANNEL	:= $(shell echo "v$(shell echo $(KERNEL_VERSION) | cut -d '.' -f 1).x")
KERNEL_ROOT		?= .tmp/linux-$(KERNEL_VERSION)

.PHONY: download-kernel
download-kernel:
ifeq (,$(wildcard $(KERNEL_ROOT)/Makefile))
	@mkdir -p $(KERNEL_ROOT) && \
		cd $(KERNEL_ROOT) && \
		curl -L https://cdn.kernel.org/pub/linux/kernel/${KERNEL_CHANNEL}/linux-${KERNEL_VERSION}.tar.xz | tar -xJf - --strip-components=1
endif

.PHONY: kernel
kernel: download-kernel
	@cd $(KERNEL_ROOT) && \
		$(MAKE) O=/target/boxos -j2

.PHONY: initramfs
initramfs:
	mkdir -pv /initramfs/{bin,sbin,etc,proc,sys,usr/{bin,sbin}}
	cp /bins/busybox /initramfs/bin/busybox
	cp ./initramfs/init.sh /initramfs/init
	chmod +x /initramfs/init
	cd /initramfs; find . -print0 | cpio --null --create --verbose --format=newc | gzip -9 > /target/boxos/initramfs
