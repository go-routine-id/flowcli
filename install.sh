#!/usr/bin/env bash
# flowcli installer — detects your platform, downloads the right prebuilt
# binary from the latest GitHub release, verifies it, and installs it.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/go-routine-id/flowcli/main/install.sh | bash
#
# Optional overrides:
#   FLOWCLI_VERSION=v0.1.0        pin a release tag instead of latest
#   FLOWCLI_INSTALL_DIR=~/bin     install somewhere else (default: /usr/local/bin)
#
# `flowcli --self-update` runs this same script with FLOWCLI_INSTALL_DIR set
# to the folder the running binary lives in.
set -euo pipefail

REPO="go-routine-id/flowcli"
BIN="flowcli"
VERSION="${FLOWCLI_VERSION:-latest}"
INSTALL_DIR="${FLOWCLI_INSTALL_DIR:-/usr/local/bin}"

info()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn()  { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
fail()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || fail "curl is required but not installed"

# --- Detect platform ---------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin)
    case "$arch" in
      arm64)          asset="$BIN-macos-arm64" ;;
      x86_64)         asset="$BIN-macos-x86_64" ;;
      *)              fail "unsupported macOS architecture: $arch" ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64|amd64)   asset="$BIN-linux-x86_64" ;;
      *)              fail "no prebuilt binary for Linux/$arch yet — build from source: https://github.com/$REPO#build-from-source" ;;
    esac
    ;;
  MINGW*|MSYS*|CYGWIN*)
    cat >&2 <<EOF
error: this script is for macOS and Linux.
On Windows, download $BIN-windows-x86_64.exe from
  https://github.com/$REPO/releases/latest
rename it to $BIN.exe and put it in a folder on your PATH.
EOF
    exit 1
    ;;
  *) fail "unsupported OS: $os" ;;
esac

info "platform: $os $arch → asset $asset"

# --- Download ----------------------------------------------------------------
if [ "$VERSION" = "latest" ]; then
  url="https://github.com/$REPO/releases/latest/download/$asset"
else
  url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

info "downloading $url"
curl -fsSL -o "$tmp/$BIN" "$url" || fail "download failed (asset $asset, version $VERSION)"

# --- Verify integrity ---------------------------------------------------------
# Every release ships <asset>.sha256 next to the binary, so a missing or
# mismatched checksum means something is wrong — both abort.
curl -fsSL -o "$tmp/$BIN.sha256" "$url.sha256" || fail "checksum file missing for $asset ($VERSION), aborting"
expected="$(awk '{print $1}' "$tmp/$BIN.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$BIN" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$tmp/$BIN" | awk '{print $1}')"
else
  fail "need sha256sum or shasum to verify the download"
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch — the download is corrupted or tampered with, aborting"
info "checksum verified (sha256)"

chmod +x "$tmp/$BIN"

# --- Install -----------------------------------------------------------------
# Use sudo only when the install dir is not writable by this user. An
# existing-but-read-only dir (e.g. /usr/local/bin on Linux) needs sudo too,
# so test writability after trying to create it, not just whether it exists.
sudo_cmd=""
mkdir -p "$INSTALL_DIR" 2>/dev/null || true
if [ ! -w "$INSTALL_DIR" ]; then
  command -v sudo >/dev/null 2>&1 || fail "cannot write to $INSTALL_DIR and sudo is unavailable — set FLOWCLI_INSTALL_DIR to a writable directory"
  sudo_cmd="sudo"
fi
$sudo_cmd mkdir -p "$INSTALL_DIR"
$sudo_cmd mv "$tmp/$BIN" "$INSTALL_DIR/$BIN"
$sudo_cmd chmod +x "$INSTALL_DIR/$BIN"

# macOS: clear the quarantine flag if present (curl usually doesn't set it,
# but Gatekeeper refuses to run the binary when it is set).
if [ "$os" = "Darwin" ]; then
  $sudo_cmd xattr -d com.apple.quarantine "$INSTALL_DIR/$BIN" 2>/dev/null || true
fi

info "installed to $INSTALL_DIR/$BIN"

# --- Verify ------------------------------------------------------------------
if "$INSTALL_DIR/$BIN" --version >/dev/null 2>&1; then
  info "version: $("$INSTALL_DIR/$BIN" --version)"
else
  warn "installed, but '$INSTALL_DIR/$BIN --version' failed — check the binary matches your platform"
fi

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) warn "$INSTALL_DIR is not on your PATH — add it, or run $BIN as $INSTALL_DIR/$BIN" ;;
esac

printf '\nRun \033[1m%s <file.mmd>\033[0m to start. Upgrade any time with \033[1m%s --self-update\033[0m.\n' "$BIN" "$BIN"
