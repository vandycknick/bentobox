GUEST_TARGET := aarch64-unknown-linux-musl
GUEST_BIN := $(CURDIR)/target/$(GUEST_TARGET)/release/bento-agent
BENTO_CONFIG := $(HOME)/.config/bento/config.yaml
ARCH ?= arm64
PROFILE ?= debug

ifeq ($(PROFILE),release)
CARGO_PROFILE_FLAGS := --release
TARGET_PROFILE_DIR := release
else ifeq ($(PROFILE),debug)
CARGO_PROFILE_FLAGS :=
TARGET_PROFILE_DIR := debug
else
$(error PROFILE must be debug or release)
endif

VMMON_BIN := target/$(TARGET_PROFILE_DIR)/vmmon

.PHONY: build-guest-agent
build-guest-agent:
	cargo zigbuild -p bento-agent --target $(GUEST_TARGET) --release
	mkdir -p "$(HOME)/.config/bento"
	printf "guest:\n  agent_binary: \"%s\"\n" "$(GUEST_BIN)" > "$(BENTO_CONFIG)"
	@echo "Updated $(BENTO_CONFIG) -> $(GUEST_BIN)"

.PHONY: build
build: vmmon
	cargo build $(CARGO_PROFILE_FLAGS) -p bentoctl

.PHONY: build-libkrun
build-libkrun:
	scripts/build-libkrun-deps

.PHONY: vmmon
vmmon:
	cargo build $(CARGO_PROFILE_FLAGS) -p bento-vmmon
	runtime/bento-vmmon/scripts/sign-vmmon "$(VMMON_BIN)"

.PHONY: kernel
kernel:
	@test -n "$(TRACK)" || (echo "TRACK is required, use TRACK=stable|longterm|longterm5" && exit 1)
	@$(MAKE) -C resources/kernels kernel TRACK=$(TRACK) ARCH=$(ARCH)

.PHONY: initramfs
initramfs: .tmp/resources-builder .tmp/busybox
	@mkdir -p ./target/resources
	@docker run \
		-v $(shell pwd)/resources:/resources \
		-v $(shell pwd)/target:/target \
		-v $(shell pwd)/.tmp:/bins \
		resources-builder \
		-C /resources/kernels initramfs TARGET_ROOT=/target/resources RESOURCE_ROOT=/resources

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
