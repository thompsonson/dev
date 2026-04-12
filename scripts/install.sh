#!/usr/bin/env bash
#
# Install the `dev` binary and, optionally, a systemd --user service
# that runs `dev daemon` in the background.
#
# Usage:
#   scripts/install.sh [--prefix DIR] [--systemd] [--uninstall]
#
# Defaults:
#   --prefix  $HOME/.local  (binary goes to $PREFIX/bin/dev)
#
# Flags:
#   --systemd   Install and enable the dev-daemon.service user unit
#               (Linux only; requires systemctl --user).
#   --uninstall Remove the binary and, if present, the systemd unit.
#
# This script is idempotent: re-running it overwrites the installed
# binary and unit file. It never touches system-wide state.

set -euo pipefail

PREFIX="${HOME}/.local"
DO_SYSTEMD=0
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
    --systemd)
      DO_SYSTEMD=1
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

log() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!! \033[0m %s\n' "$*" >&2; }
die() {
  printf '\033[1;31m!! \033[0m %s\n' "$*" >&2
  exit 1
}

repo_root() {
  cd "$(dirname "$0")/.." && pwd
}

uninstall() {
  if [[ $DO_SYSTEMD -eq 1 ]] || [[ -f "$UNIT_PATH" ]]; then
    if command -v systemctl >/dev/null 2>&1; then
      log "Stopping dev-daemon.service (if running)"
      systemctl --user stop dev-daemon.service 2>/dev/null || true
      systemctl --user disable dev-daemon.service 2>/dev/null || true
    fi
    if [[ -f "$UNIT_PATH" ]]; then
      log "Removing $UNIT_PATH"
      rm -f "$UNIT_PATH"
      systemctl --user daemon-reload 2>/dev/null || true
    fi
  fi

  if [[ -f "$BIN_PATH" ]]; then
    log "Removing $BIN_PATH"
    rm -f "$BIN_PATH"
  else
    warn "No binary at $BIN_PATH"
  fi

  log "Uninstall complete."
}

install_binary() {
  local root
  root="$(repo_root)"
  cd "$root"

  command -v cargo >/dev/null 2>&1 || die "cargo not found on PATH"
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

install_systemd_unit() {
  case "$(uname -s)" in
    Linux) ;;
    *) die "--systemd is Linux-only (got $(uname -s))" ;;
  esac
  command -v systemctl >/dev/null 2>&1 || die "systemctl not found"

  local root
  root="$(repo_root)"
  local template="${root}/contrib/systemd/dev-daemon.service"
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

if [[ $DO_SYSTEMD -eq 1 ]]; then
  install_systemd_unit
  log "Daemon is running under systemd --user. Check logs with:"
  echo "    journalctl --user -u dev-daemon.service -f"
else
  log "Binary installed. To run the daemon:"
  echo "    dev daemon"
  echo "Or install it as a systemd --user service:"
  echo "    $0 --systemd"
fi
