#!/usr/bin/env bash
# Build do LocalChat no Linux (Zorin OS, Ubuntu e derivados).
# Uso: ./scripts/build-linux.sh
set -euo pipefail

echo "==> 1/3 instalando dependencias de build (precisa de sudo)"
# sudo limpa o ambiente: sem o DEBIAN_FRONTEND aqui o tzdata trava a instalacao
# (CI, container) perguntando o fuso horario
sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential pkg-config curl ca-certificates \
    libx11-dev libxi-dev libxkbcommon-dev libxkbcommon-x11-0 libgl1-mesa-dev \
    libgtk-3-dev

echo "==> 2/3 instalando Rust (rustup), se nao existir"
if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi

echo "==> 3/3 compilando (primeira vez leva uns minutos)"
cd "$(dirname "$0")/.."
# o repo fixa toolchain e target do Windows (rust-toolchain.toml e
# .cargo/config.toml); aqui valem o toolchain e o target desta maquina
export RUSTUP_TOOLCHAIN=stable
host="$(rustc -vV | sed -n 's/^host: //p')"
cargo build --release --target "$host"
bin="target/$host/release/localchat"

echo
echo "Pronto! Binario em: $(pwd)/$bin"
echo "Rodar a interface:  ./$bin gui"
echo "Rodar so a rede:    ./$bin serve"
echo
echo "Se usar firewall: sudo ufw allow 47820/udp && sudo ufw allow 47821/tcp"
