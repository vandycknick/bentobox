#!/bin/sh
set -eu

MNT=/run/bento-cidata
SCRIPT=/run/bento-install-guest-agent.sh
DEV=/dev/disk/by-label/CIDATA

if [ ! -e "$DEV" ]; then
  DEV=/dev/disk/by-label/cidata
fi

if [ ! -e "$DEV" ]; then
  echo "bento guest agent cidata device not found" >&2
  exit 1
fi

mkdir -p -m 700 "$MNT"
# The seed disk is a VFAT NoCloud volume labeled CIDATA, so use FAT-compatible mount options
# instead of ISO-style permissions knobs like `mode=`.
mount -t vfat -o ro,uid=0,gid=0,fmask=0077,dmask=0077 "$DEV" "$MNT"
trap 'umount "$MNT" || true' EXIT

install -m 0755 "$MNT/bento-install-guest-agent.sh" "$SCRIPT"
exec "$SCRIPT"
