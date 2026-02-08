BIN_NAME := LinuxVirtualMachine
SRC := LinuxVirtualMachine/main.swift
ENTITLEMENTS := LinuxVirtualMachine/LinuxVirtualMachine.entitlements

BUILD_DIR := target
DEBUG_DIR := $(BUILD_DIR)/debug
RELEASE_DIR := $(BUILD_DIR)/release

DEBUG_BIN := $(DEBUG_DIR)/$(BIN_NAME)
RELEASE_BIN := $(RELEASE_DIR)/$(BIN_NAME)

SDK := macosx
SWIFTC := xcrun --sdk $(SDK) swiftc
CODESIGN := codesign
SIGN_IDENTITY ?= -

.PHONY: all build debug release sign-debug sign-release run clean help

all: release
build: release

debug: $(DEBUG_BIN) sign-debug

release: $(RELEASE_BIN) sign-release

$(DEBUG_BIN): $(SRC)
	mkdir -p "$(DEBUG_DIR)"
	$(SWIFTC) -g -Onone -o "$(DEBUG_BIN)" "$(SRC)"

$(RELEASE_BIN): $(SRC)
	mkdir -p "$(RELEASE_DIR)"
	$(SWIFTC) -O -o "$(RELEASE_BIN)" "$(SRC)"

sign-debug: $(DEBUG_BIN)
	$(CODESIGN) --force --sign "$(SIGN_IDENTITY)" --entitlements "$(ENTITLEMENTS)" --timestamp=none "$(DEBUG_BIN)"

sign-release: $(RELEASE_BIN)
	$(CODESIGN) --force --sign "$(SIGN_IDENTITY)" --entitlements "$(ENTITLEMENTS)" --timestamp=none "$(RELEASE_BIN)"

run: release
	@if [ -z "$(KERNEL)" ] || [ -z "$(INITRD)" ]; then \
		echo "Usage: make run KERNEL=/path/to/kernel INITRD=/path/to/initrd"; \
		exit 1; \
	fi
	"$(RELEASE_BIN)" "$(KERNEL)" "$(INITRD)"

clean:
	rm -rf "$(BUILD_DIR)"

help:
	@echo "Targets:"
	@echo "  make debug      Build and sign debug binary"
	@echo "  make release    Build and sign release binary (default)"
	@echo "  make run KERNEL=/path INITRD=/path"
	@echo "  make clean      Remove build artifacts"
	@echo ""
	@echo "Variables:"
	@echo "  SIGN_IDENTITY   codesign identity (default: - for ad-hoc)"
