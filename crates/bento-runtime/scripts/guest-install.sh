#!/bin/sh
set -eu

LOG=/var/log/bento-guest-install.log
exec >>"$LOG" 2>&1

echo "[bento] guest install start $(date -Iseconds)"

MNT=/run/bento-cidata
SRC_LOWER="$MNT/bento-instance-guest"
SRC_UPPER="$MNT/BENTO-INSTANCE-GUEST"
SRC=""

if [ -f "$SRC_LOWER" ]; then
  SRC="$SRC_LOWER"
elif [ -f "$SRC_UPPER" ]; then
  SRC="$SRC_UPPER"
else
  echo "[bento] payload not found in CIDATA mount"
  exit 1
fi

DST=/usr/local/bin/bento-instance-guest
mkdir -p "$(dirname "$DST")"

if [ -f "$DST" ] && cmp -s "$SRC" "$DST"; then
  echo "[bento] binary already up-to-date"
else
  install -m 0755 "$SRC" "$DST"
  echo "[bento] installed guest binary to $DST"
fi

cat > /etc/systemd/system/bento-instance-guest.service <<'EOF'
[Unit]
Description=Bento guest discovery agent
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/bento-instance-guest
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable bento-instance-guest.service --now
systemctl try-restart bento-instance-guest.service || systemctl start bento-instance-guest.service

echo "[bento] guest install done $(date -Iseconds)"
