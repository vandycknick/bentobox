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
