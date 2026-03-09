#!/bin/sh
set -eu

FSTAB_MARKER_START="${BENTO_FSTAB_MARKER_START:-# >>> bento managed mounts >>>}"
FSTAB_MARKER_END="${BENTO_FSTAB_MARKER_END:-# <<< bento managed mounts <<<}"
TMP_FSTAB=$(mktemp)
TMP_FRAGMENT=$(mktemp)
trap 'rm -f "$TMP_FSTAB" "$TMP_FRAGMENT"' EXIT

get_var() {
  eval "printf '%s' \"\${$1-}\""
}

escape_fstab_field() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/ /\\040/g'
}

unescape_fstab_field() {
  printf '%s' "$1" | sed 's/\\040/ /g; s/\\\\/\\/g'
}

COUNT="${BENTO_MOUNTS_COUNT:-0}"
i=0
while [ "$i" -lt "$COUNT" ]; do
  tag=$(get_var "BENTO_MOUNTS_${i}_TAG")
  path=$(get_var "BENTO_MOUNTS_${i}_PATH")
  writable=$(get_var "BENTO_MOUNTS_${i}_WRITABLE")

  if [ -z "$tag" ] || [ -z "$path" ]; then
    echo "[bento] mount entry $i missing required fields"
    exit 1
  fi

  opts="ro,nofail"
  if [ "$writable" = "true" ]; then
    opts="rw,nofail"
  fi

  escaped_path=$(escape_fstab_field "$path")
  printf '%s\t%s\tvirtiofs\t%s\t0\t0\n' "$tag" "$escaped_path" "$opts" >> "$TMP_FRAGMENT"

  i=$((i + 1))
done

if [ -f /etc/fstab ]; then
  awk -v start="$FSTAB_MARKER_START" -v end="$FSTAB_MARKER_END" '
    $0 == start { in_block = 1; next }
    $0 == end { in_block = 0; next }
    !in_block { print }
  ' /etc/fstab > "$TMP_FSTAB"
else
  : > "$TMP_FSTAB"
fi

printf '%s\n' "$FSTAB_MARKER_START" >> "$TMP_FSTAB"
cat "$TMP_FRAGMENT" >> "$TMP_FSTAB"
printf '%s\n' "$FSTAB_MARKER_END" >> "$TMP_FSTAB"

install -m 0644 "$TMP_FSTAB" /etc/fstab
echo "[bento] reconciled /etc/fstab bento-managed block"

while IFS="$(printf '\t')" read -r _spec mountpoint _fstype mountopts _freq _passno; do
  [ -n "$mountpoint" ] || continue
  mountpoint=$(unescape_fstab_field "$mountpoint")
  mkdir -p "$mountpoint"
  if mountpoint -q "$mountpoint"; then
    mount -o remount,"$mountopts" "$mountpoint"
  fi
done < "$TMP_FRAGMENT"

mount -a -t virtiofs
echo "[bento] mount reconciliation complete"
