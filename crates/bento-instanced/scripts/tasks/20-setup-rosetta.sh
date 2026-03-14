#!/bin/sh
set -eu

if [ "${BENTO_ROSETTA:-false}" != "true" ]; then
  exit 0
fi

ROSETTA_TAG=bento-rosetta
ROSETTA_MOUNT=/mnt/bento-rosetta
ROSETTA_BIN=${ROSETTA_MOUNT}/rosetta
BINFMT_ROOT=/proc/sys/fs/binfmt_misc
ROSETTA_ENTRY=${BINFMT_ROOT}/rosetta
ROSETTA_BINFMT=':rosetta:M::\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x3e\x00:\xff\xff\xff\xff\xff\xfe\xfe\x00\xff\xff\xff\xff\xff\xff\xff\xff\xfe\xff\xff\xff:/mnt/bento-rosetta/rosetta:OCF'

mkdir -p "$ROSETTA_MOUNT"
mount -t virtiofs "$ROSETTA_TAG" "$ROSETTA_MOUNT" 2>/dev/null || true

if [ ! -x "$ROSETTA_BIN" ]; then
  echo "[bento] Rosetta binary not found at $ROSETTA_BIN" >&2
  exit 1
fi

mkdir -p "$BINFMT_ROOT"
mount -t binfmt_misc binfmt_misc "$BINFMT_ROOT" 2>/dev/null || true

if [ ! -w "${BINFMT_ROOT}/register" ]; then
  echo "[bento] binfmt_misc register interface is unavailable" >&2
  exit 1
fi

if [ -f "$ROSETTA_ENTRY" ]; then
  echo -1 > "$ROSETTA_ENTRY"
fi

printf '%s' "$ROSETTA_BINFMT" > "${BINFMT_ROOT}/register"
echo "[bento] Rosetta registered for x86_64 binaries"
