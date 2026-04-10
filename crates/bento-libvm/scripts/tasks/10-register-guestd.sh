#!/bin/sh
set -eu

CONFIG_SRC="$BENTO_CIDATA_MNT/bento-guestd.yaml"
CONFIG_DST="${BENTO_GUESTD_CONFIG_PATH:-/etc/bento/guestd.yaml}"
SERVICE=/etc/systemd/system/bento-guestd.service

mkdir -p "$(dirname "$CONFIG_DST")"

CONFIG_CHANGED=1
install -m 0644 "$CONFIG_SRC" "$CONFIG_DST"
echo "[bento] reconciled guestd config at $CONFIG_DST"

cat > "$SERVICE" <<'EOF'
[Unit]
Description=Bento guest discovery agent
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/bento-guestd
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable bento-guestd.service --now

if [ "${BENTO_GUESTD_BINARY_CHANGED:-0}" -eq 1 ] || [ "$CONFIG_CHANGED" -eq 1 ]; then
  systemctl restart bento-guestd.service
else
  systemctl try-restart bento-guestd.service || systemctl start bento-guestd.service
fi

echo "[bento] guestd registration complete"
