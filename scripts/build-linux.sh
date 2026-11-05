#!/usr/bin/env bash
# Build do LocalChat no Linux (Zorin OS, Ubuntu e derivados).
# Uso: ./scripts/build-linux.sh
set -euo pipefail

echo "==> 1/3 instalando dependencias de build (precisa de sudo)"
sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends \
    build-essential pkg-config curl \
    libx11-dev libxi-dev libxkbcommon-dev libgl1-mesa-dev \
    libgtk-3-dev libdbus-1-dev libayatana-appindicator3-dev

echo "==> 2/3 instalando Rust (rustup), se nao existir"
if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi

echo "==> 3/3 compilando (primeira vez leva uns minutos)"
cd "$(dirname "$0")/.."
cargo build --release

echo
echo "Pronto! Binario em: $(pwd)/target/release/localchat"
echo "Rodar a interface:  ./target/release/localchat gui"
echo "Rodar so a rede:    ./target/release/localchat serve"
echo
echo "Se usar firewall: sudo ufw allow 47820/udp && sudo ufw allow 47821/tcp"
