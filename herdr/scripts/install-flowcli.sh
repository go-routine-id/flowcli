#!/usr/bin/env bash
# Plugin build step: download the flowcli release that matches this
# checkout's Cargo.toml version into <plugin root>/bin. Herdr runs it from
# the plugin root, with the whole flowcli repo checked out around it.
set -euo pipefail

plugin_root="$(cd "$(dirname "$0")/.." && pwd)"
repo_root="$(cd "$plugin_root/.." && pwd)"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo_root/Cargo.toml" | head -n 1)"
if [ -z "$version" ]; then
  echo "error: cannot read the flowcli version from $repo_root/Cargo.toml" >&2
  exit 1
fi

# install.sh pins the tag, verifies the .sha256 and installs without sudo
# because bin/ is ours. A version with no published release yet fails
# here with the asset name and tag in the message.
FLOWCLI_VERSION="v$version" FLOWCLI_INSTALL_DIR="$plugin_root/bin" \
  bash "$repo_root/install.sh"
