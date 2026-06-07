#!/usr/bin/env bash
#
# Install the `dev` binary in one of two roles:
#
#   host    — this machine runs `dev daemon` (the tmux control plane).
#             On Linux this installs + enables a systemd --user unit.
#             This is your always-on box (e.g. pop-mini).
#
#   client  — this machine drives sessions on a host over SSH.
#             Installs the binary and records `default_host=HOST` in
#             ~/.config/dev/config so `dev <project>` targets that host.
#             Use this on your laptop and phone (Termux).
#
# With no role flag it just installs the binary (back-compat).
#
# Usage:
#   scripts/install.sh [--prefix DIR]
#   scripts/install.sh --host                 # daemon host (pop-mini)
#   scripts/install.sh --client HOST          # client pointing at HOST
#   scripts/install.sh --uninstall
#
# Defaults:
#   --prefix  $HOME/.local  (binary goes to $PREFIX/bin/dev)
#
# Flags:
#   --host          Install + enable the dev-daemon.service user unit
#                   (Linux only; requires systemctl --user).
#   --client HOST   Client role: write default_host=HOST, no daemon.
#   --systemd       Deprecated alias for --host.
#   --uninstall     Remove the binary and, if present, the systemd unit.
#
# This script is idempotent: re-running it overwrites the installed
# binary, unit file, and config keys. It never touches system-wide state.

set -euo pipefail

PREFIX="${HOME}/.local"
ROLE=""            # "" | host | client
CLIENT_HOST=""
DO_UNINSTALL=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      PREFIX="$2"
      shift 2
      ;;
    --prefix=*)
      PREFIX="${1#--prefix=}"
      shift
      ;;
    --host|--systemd)
      ROLE="host"
      shift
      ;;
    --client)
      ROLE="client"
      CLIENT_HOST="${2:-}"
      [[ -n "$CLIENT_HOST" ]] || { echo "--client requires a HOST argument" >&2; exit 1; }
      shift 2
      ;;
    --client=*)
      ROLE="client"
      CLIENT_HOST="${1#--client=}"
      [[ -n "$CLIENT_HOST" ]] || { echo "--client requires a HOST argument" >&2; exit 1; }
      shift
      ;;
    --uninstall)
      DO_UNINSTALL=1
      shift
      ;;
    -h|--help)
      awk 'NR==1 {next} /^[^#]/ {exit} {sub(/^# ?/, ""); print}' "$0"
      exit 0
      ;;
    *)
      echo "unknown flag: $1" >&2
      exit 1
      ;;
  esac
done

BIN_DIR="${PREFIX}/bin"
BIN_PATH="${BIN_DIR}/dev"
UNIT_DIR="${HOME}/.config/systemd/user"
UNIT_PATH="${UNIT_DIR}/dev-daemon.service"
CONFIG_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/dev"
CONFIG_FILE="${CONFIG_DIR}/config"

log() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!! \033[0m %s\n' "$*" >&2; }
die() {
  printf '\033[1;31m!! \033[0m %s\n' "$*" >&2
  exit 1
}

repo_root() {
  cd "$(dirname "$0")/.." && pwd
}

# Termux (Android) has no systemd and a non-standard prefix, but ~/.local/bin
# is writable and on PATH, so the binary install path is unchanged.
is_termux() {
  [[ -n "${TERMUX_VERSION:-}" ]] ||
    [[ "$(uname -o 2>/dev/null)" == "Android" ]] ||
    [[ -d /data/data/com.termux ]]
}

uninstall() {
  if [[ -f "$UNIT_PATH" ]]; then
    if command -v systemctl >/dev/null 2>&1; then
      log "Stopping dev-daemon.service (if running)"
      systemctl --user stop dev-daemon.service 2>/dev/null || true
      systemctl --user disable dev-daemon.service 2>/dev/null || true
    fi
    log "Removing $UNIT_PATH"
    rm -f "$UNIT_PATH"
    systemctl --user daemon-reload 2>/dev/null || true
  fi

  if [[ -f "$BIN_PATH" ]]; then
    log "Removing $BIN_PATH"
    rm -f "$BIN_PATH"
  else
    warn "No binary at $BIN_PATH"
  fi

  log "Uninstall complete. (Left $CONFIG_FILE untouched.)"
}

install_binary() {
  local root
  root="$(repo_root)"
  cd "$root"

  if ! command -v cargo >/dev/null 2>&1; then
    if is_termux; then
      die "cargo not found. On Termux: pkg install rust tmux openssh, then re-run."
    fi
    die "cargo not found on PATH"
  fi
  command -v tmux >/dev/null 2>&1 || warn "tmux not found on PATH — dev will fail at runtime without it"

  log "Building release binary ($(cargo --version))"
  cargo build --release --quiet

  local src="${root}/target/release/dev"
  [[ -x "$src" ]] || die "build did not produce $src"

  mkdir -p "$BIN_DIR"
  log "Installing $src -> $BIN_PATH"
  install -m 0755 "$src" "$BIN_PATH"

  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR is not on your PATH. Add it to your shell rc:"
       warn "    export PATH=\"$BIN_DIR:\$PATH\""
       ;;
  esac
}

# Idempotently set `key=value` in the dev config (INI-style, one per line).
set_config_key() {
  local key="$1" val="$2"
  mkdir -p "$CONFIG_DIR"
  touch "$CONFIG_FILE"
  if grep -qE "^${key}=" "$CONFIG_FILE" 2>/dev/null; then
    local tmp
    tmp="$(mktemp)"
    sed "s|^${key}=.*|${key}=${val}|" "$CONFIG_FILE" >"$tmp"
    mv "$tmp" "$CONFIG_FILE"
  else
    printf '%s=%s\n' "$key" "$val" >>"$CONFIG_FILE"
  fi
  log "Set ${key}=${val} in $CONFIG_FILE"
}

install_systemd_unit() {
  case "$(uname -s)" in
    Linux) ;;
    *) die "host role needs systemd (Linux only); got $(uname -s)" ;;
  esac
  is_termux && die "host role is not supported on Termux — your phone is a client, not the daemon host"
  command -v systemctl >/dev/null 2>&1 || die "systemctl not found"

  local root template
  root="$(repo_root)"
  template="${root}/contrib/systemd/dev-daemon.service"
  [[ -f "$template" ]] || die "missing unit template: $template"

  mkdir -p "$UNIT_DIR"
  log "Installing user unit -> $UNIT_PATH"
  install -m 0644 "$template" "$UNIT_PATH"

  log "Reloading systemd user units"
  systemctl --user daemon-reload

  log "Enabling and starting dev-daemon.service"
  systemctl --user enable --now dev-daemon.service

  log "Unit status:"
  systemctl --user --no-pager status dev-daemon.service || true
}

if [[ $DO_UNINSTALL -eq 1 ]]; then
  uninstall
  exit 0
fi

install_binary

case "$ROLE" in
  host)
    install_systemd_unit
    log "Host ready. Daemon is running under systemd --user. Logs:"
    echo "    journalctl --user -u dev-daemon.service -f"
    ;;
  client)
    set_config_key "default_host" "$CLIENT_HOST"
    log "Client ready. 'dev <project>' will target ${CLIENT_HOST} over SSH."
    echo "Check reachability:  ssh ${CLIENT_HOST} true"
    ;;
  *)
    log "Binary installed (no role). To run the daemon here:"
    echo "    dev daemon"
    echo "Or pick a role:"
    echo "    $0 --host            # this machine is the daemon host"
    echo "    $0 --client pop-mini # this machine is a client"
    ;;
esac
