#!/bin/sh
# Instala (ou reinstala) o LocalChat no Linux sem compilar nada:
#
#   wget -qO- https://raw.githubusercontent.com/hardeath950/localchat/main/scripts/install-linux.sh | sh
#
# Baixa a versao mais recente do GitHub, confere o SHA-256, instala na sua pasta
# de usuario (sem sudo; o update automatico consegue se atualizar ai), cria o
# icone no menu de aplicativos e, se voce quiser, inicia junto com o computador.
set -eu

REPO="hardeath950/localchat"
# LOCALCHAT_RELEASE_URL: so para testar com um servidor local
BASE="${LOCALCHAT_RELEASE_URL:-https://github.com/$REPO/releases/latest/download}"
DIR="$HOME/.local/share/localchat"
APPS="$HOME/.local/share/applications"
AUTOSTART="$HOME/.config/autostart"

say() { printf '%s\n' "$*"; }
die() { say "Erro: $*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || die "este instalador e para Linux."
[ "$(uname -m)" = x86_64 ] || die "so ha versao para PC 64 bits (x86_64)."

# wget vem no Ubuntu/Zorin; curl nem sempre
if command -v wget >/dev/null 2>&1; then
    get() { wget -q -O "$2" "$1"; }
elif command -v curl >/dev/null 2>&1; then
    get() { curl -fsSL -o "$2" "$1"; }
else
    die "precisa do wget ou do curl (sudo apt install wget)."
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "==> Baixando o LocalChat..."
get "$BASE/localchat-linux-x86_64" "$tmp/localchat-linux-x86_64" || die "download falhou (sem internet?)."
get "$BASE/SHA256SUMS" "$tmp/SHA256SUMS" || die "download falhou (sem internet?)."
(cd "$tmp" && grep ' localchat-linux-x86_64$' SHA256SUMS | sha256sum -c --status) \
    || die "o arquivo baixado nao confere (SHA-256 diferente). Tente de novo."

mkdir -p "$DIR" "$APPS"
install -m 755 "$tmp/localchat-linux-x86_64" "$DIR/localchat"

cat > "$DIR/localchat.svg" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><path d="M9 3h14a7 7 0 0 1 7 7v9a7 7 0 0 1-7 7H16l-6 4.5V26H9a7 7 0 0 1-7-7v-9a7 7 0 0 1 7-7z" fill="#2563eb"/><circle cx="10.5" cy="14.5" r="2" fill="#fff"/><circle cx="16" cy="14.5" r="2" fill="#fff"/><circle cx="21.5" cy="14.5" r="2" fill="#fff"/></svg>
SVG

cat > "$APPS/localchat.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=LocalChat
Comment=Chat e arquivos entre os computadores da rede local
Exec="$DIR/localchat" gui
Icon=$DIR/localchat.svg
Terminal=false
Categories=Network;Chat;
EOF

# bibliotecas da janela; o Ubuntu/Zorin com area de trabalho ja tem as duas
missing=""
ldconfig -p 2>/dev/null | grep -q 'libgtk-3\.so\.0' || missing="$missing libgtk-3-0"
ldconfig -p 2>/dev/null | grep -q 'libxkbcommon-x11\.so\.0' || missing="$missing libxkbcommon-x11-0"
if [ -n "$missing" ]; then
    say "==> Instalando o que falta:$missing (vai pedir sua senha)..."
    # shellcheck disable=SC2086
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y $missing \
        || say "Aviso: rode depois: sudo apt install$missing"
fi

# a pergunta vai pelo terminal (o script chega pelo pipe do wget); sem terminal, sim
answer=s
if (: < /dev/tty) 2>/dev/null; then
    printf 'Iniciar o LocalChat junto com o computador? [S/n] ' > /dev/tty
    read -r answer < /dev/tty || answer=s
fi
case "$answer" in
    [nN]*) rm -f "$AUTOSTART/localchat.desktop" ;;
    *) mkdir -p "$AUTOSTART" && cp "$APPS/localchat.desktop" "$AUTOSTART/localchat.desktop" ;;
esac

if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q 'Status: active'; then
    say "Firewall ligado: libere o LocalChat com"
    say "  sudo ufw allow 47820/udp && sudo ufw allow 47821/tcp"
fi

say ""
say "Pronto! O LocalChat esta no menu de aplicativos."
say "Para desinstalar: rm -rf \"$DIR\" \"$APPS/localchat.desktop\" \"$AUTOSTART/localchat.desktop\""

# abre agora (se ja estiver aberto, so mostra a janela)
if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    nohup "$DIR/localchat" gui >/dev/null 2>&1 &
fi
