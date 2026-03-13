TRACK ?=
ARCH ?= arm64
KERNEL_VERSION ?=
TARGET_ROOT ?= /target/resources
KERNEL_TARGET_ROOT := $(TARGET_ROOT)/kernels
KERNEL_CHANNEL := v$(shell printf '%s' "$(KERNEL_VERSION)" | cut -d '.' -f 1).x
KERNEL_SOURCE_DIR ?= $(KERNEL_ROOT)
KERNEL_BUILD_DIR ?= $(KERNEL_TARGET_ROOT)/build/$(TRACK)-$(ARCH)-$(KERNEL_VERSION)
KERNEL_OUT_DIR ?= $(KERNEL_TARGET_ROOT)/out/$(TRACK)-$(ARCH)-$(KERNEL_VERSION)
BASE_CONFIG ?= /resources/kernels/configs/$(ARCH)-base.config
INITRAMFS_OUT_DIR ?= $(TARGET_ROOT)/initramfs

.PHONY: kernel
kernel: guard-kernel
	@mkdir -p "$(KERNEL_SOURCE_DIR)" "$(KERNEL_BUILD_DIR)" "$(KERNEL_OUT_DIR)"
	@if [ ! -f "$(KERNEL_SOURCE_DIR)/Makefile" ]; then \
		curl -L "https://cdn.kernel.org/pub/linux/kernel/$(KERNEL_CHANNEL)/linux-$(KERNEL_VERSION).tar.xz" | tar -xJf - --strip-components=1 -C "$(KERNEL_SOURCE_DIR)"; \
	fi
	@cp "$(BASE_CONFIG)" "$(KERNEL_BUILD_DIR)/.config"
	@$(MAKE) -C "$(KERNEL_SOURCE_DIR)" O="$(KERNEL_BUILD_DIR)" ARCH="$(ARCH)" olddefconfig
	@$(MAKE) -C "$(KERNEL_SOURCE_DIR)" O="$(KERNEL_BUILD_DIR)" ARCH="$(ARCH)" -j2 Image
	@cp "$(KERNEL_BUILD_DIR)/.config" "$(KERNEL_OUT_DIR)/.config"
	@cp "$(KERNEL_BUILD_DIR)/System.map" "$(KERNEL_OUT_DIR)/System.map"
	@cp "$(KERNEL_BUILD_DIR)/arch/$(ARCH)/boot/Image" "$(KERNEL_OUT_DIR)/Image"

.PHONY: initramfs
initramfs:
	@mkdir -p /initramfs/bin /initramfs/sbin /initramfs/etc /initramfs/proc /initramfs/sys /initramfs/usr/bin /initramfs/usr/sbin "$(INITRAMFS_OUT_DIR)"
	@cp /bins/busybox /initramfs/bin/busybox
	@cp /resources/initramfs/init.sh /initramfs/init
	@chmod +x /initramfs/init
	@cd /initramfs && find . -print0 | cpio --null --create --verbose --format=newc | gzip -9 > "$(INITRAMFS_OUT_DIR)/initramfs"

.PHONY: guard-kernel
guard-kernel:
	@test -n "$(TRACK)" || (echo "TRACK is required" && exit 1)
	@test -n "$(KERNEL_VERSION)" || (echo "KERNEL_VERSION is required" && exit 1)
	@test -n "$(ARCH)" || (echo "ARCH is required" && exit 1)
	@test -f "$(BASE_CONFIG)" || (echo "missing base config $(BASE_CONFIG)" && exit 1)
