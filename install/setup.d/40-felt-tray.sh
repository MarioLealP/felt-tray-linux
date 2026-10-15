#!/bin/bash
# felt-tray's per-user setup (see clockmost-desktop-setup).
set -eu

cfg="${XDG_CONFIG_HOME:-$HOME/.config}/felt-tray/config.toml"
if [[ ! -e $cfg ]]; then
    install -Dm644 /usr/share/felt-tray/config.clockmost.toml "$cfg"
    echo "   felt-tray config written ($cfg)"
fi

systemctl --user daemon-reload
systemctl --user enable felt-tray.service >/dev/null 2>&1 && echo "   felt-tray starts with the session"

if ! id -nG | tr ' ' '\n' | grep -qx tray-certs; then
    echo "   felt-tray: this session is not in group tray-certs yet - install the vault renewer" \
        "(clockmost-infra projects/openbao/offhost/install-laptop.sh <user> tray), then log in again"
fi
