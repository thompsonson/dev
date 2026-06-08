#!/usr/bin/env bash
#
# Download and install a prebuilt `dev` binary from GitHub Releases.
# No git clone, no cargo — handy for Termux on a phone/tablet.
#
# Quick start:
#   curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | bash
#   curl -fsSL .../bootstrap.sh | DEV_HOST=pop-mini bash          # install as a client of pop-mini
#   curl -fsSL .../bootstrap.sh | DEV_CHANNEL=dev bash            # install latest dev build
#
# Or, run as a file with flags:
#   bootstrap.sh --client pop-mini
#   bootstrap.sh --host
#   bootstrap.sh --channel dev
#   bootstrap.sh --version v0.1.0 --prefix ~/.local
#
# Env vars (flags override): DEV_HOST, DEV_ROLE, DEV_VERSION, DEV_CHANNEL, DEV_PREFIX
# Setting DEV_HOST implies the client role.

set -euo pipefail

REPO="thompsonson/dev"
ROLE="${DEV_ROLE:-}"
CLIENT_HOST="${DEV_HOST:-}"
VERSION="${DEV_VERSION:-latest}"
CHANNEL="${DEV_CHANNEL:-stable}"
INSTALL_PREFIX="${DEV_PREFIX:-}"

[[ -n "$CLIENT_HOST" && -z "$ROLE" ]] && ROLE="client"

usage() {
  cat <<'EOF'
bootstrap.sh — download + install a prebuilt `dev` binary

  --host           install + enable the systemd --user daemon (Linux host)
  --client HOST    install + record default_host=HOST (laptop/phone)
  --channel CH     release channel: stable (default) or dev
  --version V      release tag to install (default: latest for channel)
  --prefix DIR     install prefix (default: Termux $PREFIX, else ~/.local)
  -h, --help       this help

Env: DEV_HOST, DEV_ROLE, DEV_CHANNEL, DEV_VERSION, DEV_PREFIX
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --host) ROLE="host"; shift;;
    --client) ROLE="client"; CLIENT_HOST="${2:-}"; [[ -n "$CLIENT_HOST" ]] || { echo "--client needs HOST" >&2; exit 1; }; shift 2;;
    --client=*) ROLE="client"; CLIENT_HOST="${1#--client=}"; shift;;
    --channel) CHANNEL="${2:?}"; shift 2;;
    --channel=*) CHANNEL="${1#--channel=}"; shift;;
    --version) VERSION="${2:?}"; shift 2;;
    --version=*) VERSION="${1#--version=}"; shift;;
    --prefix) INSTALL_PREFIX="${2:?}"; shift 2;;
    --prefix=*) INSTALL_PREFIX="${1#--prefix=}"; shift;;
    -h|--help) usage; exit 0;;
    *) echo "unknown flag: $1" >&2; exit 1;;
  esac
done

log() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!! \033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31m!! \033[0m %s\n' "$*" >&2; exit 1; }

is_termux() {
  [[ -n "${TERMUX_VERSION:-}" ]] ||
    [[ "$(uname -o 2>/dev/null)" == "Android" ]] ||
    [[ -d /data/data/com.termux ]]
}

# --- pick install prefix -----------------------------------------------------
# On Termux, $PREFIX/bin is already on PATH; elsewhere use ~/.local/bin.
if [[ -z "$INSTALL_PREFIX" ]]; then
  if is_termux && [[ -n "${PREFIX:-}" ]]; then
    INSTALL_PREFIX="$PREFIX"
  else
    INSTALL_PREFIX="$HOME/.local"
  fi
fi
BIN_DIR="$INSTALL_PREFIX/bin"

# --- detect target -----------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)
    if is_termux; then
      plat="linux-android"   # NDK-built binary, works natively on Android/Termux
    else
      plat="unknown-linux-musl"
    fi
    ;;
  Darwin) plat="apple-darwin" ;;
  *) die "unsupported OS: $os" ;;
esac
case "$arch" in
  x86_64|amd64) cpu="x86_64" ;;
  aarch64|arm64) cpu="aarch64" ;;
  *) die "unsupported arch: $arch" ;;
esac
TRIPLE="${cpu}-${plat}"
ASSET="dev-${TRIPLE}.tar.gz"

# Resolve version. Always use a specific tag so the download hits the
# versioned URL directly — the releases/latest/download redirect is prone
# to 504s on GitHub's CDN.
api_fetch() { # url → stdout
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "$1"
  else
    die "need curl or wget"
  fi
}

if [[ "$VERSION" == "latest" ]]; then
  case "$CHANNEL" in
    stable)
      VERSION="$(api_fetch "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
      [[ -n "$VERSION" ]] || die "could not resolve latest stable release tag"
      ;;
    dev)
      # Pick the newest pre-release. Requires python3 or jq for reliable JSON
      # parsing; falls back to grep (may break if GitHub changes field ordering).
      raw="$(api_fetch "https://api.github.com/repos/${REPO}/releases")"
      if command -v python3 >/dev/null 2>&1; then
        VERSION="$(printf '%s' "$raw" | python3 -c "
import sys, json
for r in json.load(sys.stdin):
    if r.get('prerelease'):
        print(r['tag_name']); sys.exit(0)
sys.exit(1)")" || die "no dev release found"
      elif command -v jq >/dev/null 2>&1; then
        VERSION="$(printf '%s' "$raw" | jq -r '[.[]|select(.prerelease)][0].tag_name')"
        [[ "$VERSION" != "null" && -n "$VERSION" ]] || die "no dev release found"
      else
        # Dev tags always contain -dev. in the name — filter tag_name lines by
        # that pattern. The || true prevents set -e from silently exiting when
        # grep finds no matches; the empty-VERSION check below surfaces the error.
        VERSION="$(printf '%s' "$raw" | grep '"tag_name"' | grep -- '-dev\.' \
          | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/' || true)"
        [[ -n "$VERSION" ]] || die "no dev release found (install python3 or jq for reliable resolution)"
      fi
      ;;
    *) die "unknown channel: $CHANNEL (use stable or dev)";;
  esac
fi
BASE="https://github.com/${REPO}/releases/download/${VERSION}"
URL="${BASE}/${ASSET}"

# --- download + verify + install --------------------------------------------
fetch() { # url outfile
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 --retry-delay 2 "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    die "need curl or wget"
  fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

log "Downloading ${ASSET} (${VERSION})"
fetch "$URL" "$tmp/$ASSET" || die "download failed: $URL"

if fetch "$URL.sha256" "$tmp/$ASSET.sha256" 2>/dev/null; then
  want="$(awk '{print $1}' "$tmp/$ASSET.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    got="$(sha256sum "$tmp/$ASSET" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    got="$(shasum -a 256 "$tmp/$ASSET" | awk '{print $1}')"
  else
    got=""
  fi
  if [[ -n "$got" && "$got" != "$want" ]]; then
    die "checksum mismatch (got $got, want $want)"
  fi
  [[ -n "$got" ]] && log "checksum OK"
else
  warn "no checksum published; skipping verification"
fi

tar -xzf "$tmp/$ASSET" -C "$tmp"
binsrc="$(find "$tmp" -type f -name dev | head -n1)"
[[ -n "$binsrc" ]] || die "binary 'dev' not found in $ASSET"

mkdir -p "$BIN_DIR"
install -m 0755 "$binsrc" "$BIN_DIR/dev" 2>/dev/null || { cp "$binsrc" "$BIN_DIR/dev"; chmod 0755 "$BIN_DIR/dev"; }
log "Installed -> $BIN_DIR/dev"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR is not on PATH. Add: export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
command -v tmux >/dev/null 2>&1 || warn "tmux not found — $(is_termux && echo 'pkg install tmux openssh' || echo 'install tmux via your package manager')"

# --- role setup --------------------------------------------------------------
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/dev"
CONFIG_FILE="$CONFIG_DIR/config"
set_config_key() {
  local key="$1" val="$2"
  mkdir -p "$CONFIG_DIR"
  touch "$CONFIG_FILE"
  if grep -qE "^${key}=" "$CONFIG_FILE" 2>/dev/null; then
    local t; t="$(mktemp)"
    sed "s|^${key}=.*|${key}=${val}|" "$CONFIG_FILE" >"$t"
    mv "$t" "$CONFIG_FILE"
  else
    printf '%s=%s\n' "$key" "$val" >>"$CONFIG_FILE"
  fi
  log "Set ${key}=${val} in $CONFIG_FILE"
}

case "$ROLE" in
  client)
    [[ -n "$CLIENT_HOST" ]] || die "client role needs a host (DEV_HOST=... or --client HOST)"
    set_config_key "default_host" "$CLIENT_HOST"
    log "Client of ${CLIENT_HOST}. Check reachability:  ssh ${CLIENT_HOST} true"
    ;;
  host)
    is_termux && die "host role is not supported on Termux — a phone is a client"
    command -v systemctl >/dev/null 2>&1 || die "host role needs systemd --user"
    UNIT_DIR="$HOME/.config/systemd/user"
    mkdir -p "$UNIT_DIR"
    cat >"$UNIT_DIR/dev-daemon.service" <<EOF
[Unit]
Description=dev tmux control-plane daemon
Documentation=https://github.com/${REPO}
After=default.target

[Service]
Type=simple
ExecStart=${BIN_DIR}/dev daemon
Restart=on-failure
RestartSec=2
Environment=PATH=${BIN_DIR}:/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable --now dev-daemon.service
    log "Daemon running. Logs:  journalctl --user -u dev-daemon.service -f"
    ;;
  *)
    log "Binary installed (no role). Pick one next:"
    echo "    dev daemon                 # run the host daemon directly, or"
    echo "    bootstrap.sh --host        # systemd host (pop-mini)"
    echo "    bootstrap.sh --client pop-mini"
    ;;
esac
