KERNEL_VERSION	:= 6.6.72

.PHONY: build-kernel
build-kernel: .tmp/boxos-builder
	@docker volume create kernel-$(KERNEL_VERSION)-cache
	@mkdir -p ./target/boxos
	@docker run -it --mount source=kernel-$(KERNEL_VERSION)-cache,target=/kernel \
		-e KERNEL_VERSION=$(KERNEL_VERSION) \
		-e KERNEL_ROOT=/kernel \
		-v $(shell pwd)/boxos:/boxos \
		-v $(shell pwd)/target:/target \
		boxos-builder \
		kernel

.PHONY: build-initramfs
build-initramfs: .tmp/boxos-builder .tmp/busybox
	@mkdir -p ./target/boxos
	@docker run \
		-v $(shell pwd)/boxos:/boxos \
		-v $(shell pwd)/target:/target \
		-v $(shell pwd)/.tmp:/bins \
		boxos-builder \
		initramfs

.PHONY: build-rootfs
build-rootfs:
	@docker build -f boxos/rootfs/Dockerfile -t rootfs .
	@docker run -it  -v $(shell pwd)/target/boxos:/boxos --privileged --cap-add=CAP_MKNOD rootfs

.tmp/boxos-builder: boxos/Containerfile
	@docker build -f boxos/Containerfile -t boxos-builder .
	@touch .tmp/boxos-builder

.tmp/busybox: boxos/busybox/Containerfile
	@cd boxos/busybox && \
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
