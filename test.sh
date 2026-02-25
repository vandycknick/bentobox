sudo install -d /run/systemd/network
sudo tee /run/systemd/network/10-vznat-test.network >/dev/null <<'EOF'
[Match]
Name=enp0s1

[Network]
DHCP=yes
EOF

sudo networkctl reload
sudo networkctl reconfigure enp0s1

resolvectl status
systemctl is-enabled systemd-resolved
systemctl is-active systemd-resolved
ls -l /etc/resolv.conf
ip route


sudo rm -f /etc/resolv.conf
     sudo ln -s /run/systemd/resolve/stub-resolv.conf /etc/resolv.conf
     sudo systemctl restart systemd-resolved
     sudo resolvectl status
     cat /etc/resolv.conf
