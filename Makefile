GUEST_TARGET := aarch64-unknown-linux-musl
GUEST_BIN := $(CURDIR)/target/$(GUEST_TARGET)/release/bento-guestd
BENTO_CONFIG := $(HOME)/.config/bento/config.yaml
BENTO_PROFILE_DIR := $(HOME)/.config/bento/profiles
ARCH ?= arm64

.PHONY: build-guest-agent
build-guest-agent:
	cargo zigbuild -p bento-guestd --target $(GUEST_TARGET) --release
	mkdir -p "$(HOME)/.config/bento"
	printf "guest:\n  agent_binary: \"%s\"\n" "$(GUEST_BIN)" > "$(BENTO_CONFIG)"
	@echo "Updated $(BENTO_CONFIG) -> $(GUEST_BIN)"

.PHONY: sync-profiles
sync-profiles:
	mkdir -p "$(BENTO_PROFILE_DIR)"
	cp config/profiles/*.yaml "$(BENTO_PROFILE_DIR)/"
	@echo "Synced profiles to $(BENTO_PROFILE_DIR)"


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

.PHONY: debug
debug:
	cargo build -p bentoctl -p bento-vmmon
	codesign -f --entitlement ./app.entitlements -s - target/debug/bentoctl
	codesign -f --entitlement ./app.entitlements -s - target/debug/vmmon
	./target/debug/bentoctl create macos:15.0.1 -a arm64
	# truncate -s 34g ./disk.img
	# truncate -s 34m ./aux.img

.PHONY: debug-start
debug-start:
	cargo build -p bentoctl -p bento-vmmon
	codesign -f --entitlement ./app.entitlements -s - target/debug/bentoctl
	codesign -f --entitlement ./app.entitlements -s - target/debug/vmmon
	./target/debug/bentoctl start macos


.PHONY: linux
linux:
	cargo build --bin linux
	codesign -f --entitlement ./app.entitlements -s - target/debug/linux
	RUST_BACKTRACE=full ./target/debug/linux
