#!/bin/sh
# The plugin pane: flowcli on $FLOWCLI_FILE, redrawn on every save, with
# selections appended to $FLOWCLI_EVENTS (default <file>.events.jsonl,
# next to the diagram like the .layout sidecar, so an agent that knows the
# diagram path also knows where to read).
set -u

plugin_root="$(cd "$(dirname "$0")/.." && pwd)"

# Keep the pane open on a setup error, so the message can be read.
hold() {
  printf 'flowcli: %s\n\nPress Enter to close this pane.' "$1"
  read -r _
  exit 1
}

file="${FLOWCLI_FILE:-}"
[ -n "$file" ] || hold "no diagram given — open the pane with --env FLOWCLI_FILE=/absolute/path.mmd"
case "$file" in
  /*) ;;
  *) hold "FLOWCLI_FILE must be an absolute path (got: $file)" ;;
esac
[ -f "$file" ] || hold "$file does not exist"

bin="$plugin_root/bin/flowcli"
if [ ! -x "$bin" ]; then
  bin="$(command -v flowcli 2>/dev/null)" ||
    hold "no flowcli binary — run: bash $plugin_root/scripts/install-flowcli.sh"
fi
"$bin" --help 2>&1 | grep -q -- '--watch' ||
  hold "$bin is too old for --watch/--events — run: bash $plugin_root/scripts/install-flowcli.sh"

events="${FLOWCLI_EVENTS:-$file.events.jsonl}"
exec "$bin" "$file" --watch --events "$events"
