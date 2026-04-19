#!/bin/sh
set -eu

CONFIG_SRC="$BENTO_CIDATA_MNT/bento-agent.yaml"
CONFIG_DST="${BENTO_AGENT_CONFIG_PATH:-/etc/bento/agent.yaml}"
SERVICE=/etc/systemd/system/bento-agent.service

mkdir -p "$(dirname "$CONFIG_DST")"

install -m 0644 "$CONFIG_SRC" "$CONFIG_DST"
echo "[bento] reconciled agent config at $CONFIG_DST"

cat > "$SERVICE" <<'EOF'
[Unit]
Description=Bento guest agent
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/bento-agent
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable bento-agent.service

if systemctl is-active --quiet bento-agent.service; then
  systemctl restart bento-agent.service
else
  systemctl start bento-agent.service
fi

echo "[bento] agent registration complete"
