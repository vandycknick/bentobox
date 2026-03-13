GUEST_TARGET := aarch64-unknown-linux-musl
GUEST_BIN := $(CURDIR)/target/$(GUEST_TARGET)/release/bento-guestd
BENTO_CONFIG := $(HOME)/.config/bento/config.yaml
ARCH ?= arm64

ifeq ($(origin KERNEL_VERSION), undefined)
ifeq ($(TRACK),stable)
KERNEL_VERSION := 6.19.7
else ifeq ($(TRACK),longterm)
KERNEL_VERSION := 6.18.17
else ifeq ($(TRACK),longterm5)
KERNEL_VERSION := 5.15.202
endif
endif

.PHONY: build-guest-agent
build-guest-agent:
	cargo zigbuild -p bento-guestd --target $(GUEST_TARGET) --release
	mkdir -p "$(HOME)/.config/bento"
	printf "guest:\n  agent_binary: \"%s\"\n" "$(GUEST_BIN)" > "$(BENTO_CONFIG)"
	@echo "Updated $(BENTO_CONFIG) -> $(GUEST_BIN)"


.PHONY: kernel
kernel: .tmp/resources-builder
	@test -n "$(TRACK)" || (echo "TRACK is required, use TRACK=stable|longterm|longterm5" && exit 1)
	@test -n "$(KERNEL_VERSION)" || (echo "unsupported TRACK '$(TRACK)'" && exit 1)
	@mkdir -p ./target/resources
	@docker run -it \
		-e KERNEL_VERSION=$(KERNEL_VERSION) \
		-e KERNEL_ROOT=/target/resources/kernels/src/linux-$(KERNEL_VERSION) \
		-e TRACK=$(TRACK) \
		-e ARCH=$(ARCH) \
		-v $(shell pwd)/resources:/resources \
		-v $(shell pwd)/target:/target \
		resources-builder \
		-C /resources/kernels kernel

.PHONY: initramfs
initramfs: .tmp/resources-builder .tmp/busybox
	@mkdir -p ./target/resources
	@docker run \
		-v $(shell pwd)/resources:/resources \
		-v $(shell pwd)/target:/target \
		-v $(shell pwd)/.tmp:/bins \
		resources-builder \
		-C /resources/kernels initramfs

.PHONY: rootfs
rootfs:
	@mkdir -p ./target/resources/rootfs
	@docker build -f resources/rootfs/Dockerfile -t rootfs .
	@docker run -it -v $(shell pwd)/target/resources/rootfs:/resources --privileged --cap-add=CAP_MKNOD rootfs

.tmp/resources-builder: resources/Containerfile
	@docker build -f resources/Containerfile -t resources-builder .
	@touch .tmp/resources-builder

.tmp/busybox: resources/busybox/Containerfile
	@cd resources/busybox && \
		docker build -f Containerfile -t busybox-builder .
	@docker run -v $(shell pwd)/.tmp:/output \
			busybox-builder \
			cp /build/busybox /output

.PHONY: debug
debug:
	cargo build
	codesign -f --entitlement ./app.entitlements -s - target/debug/bento
	./target/debug/bento create macos:15.0.1 -a arm64
	# truncate -s 34g ./disk.img
	# truncate -s 34m ./aux.img

.PHONY: debug-start
debug-start:
	cargo build
	codesign -f --entitlement ./app.entitlements -s - target/debug/bento
	./target/debug/bento start macos


.PHONY: linux
linux:
	cargo build --bin linux
	codesign -f --entitlement ./app.entitlements -s - target/debug/linux
	RUST_BACKTRACE=full ./target/debug/linux
