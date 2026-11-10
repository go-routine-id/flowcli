#!/usr/bin/env bash
# Action "open-diagram": open the most recently changed .mmd in the
# focused pane's directory in a flowcli pane to its right.
set -euo pipefail

herdr="${HERDR_BIN_PATH:-herdr}"

# Herdr passes the invocation context as JSON; only one plain string field
# is needed, so a sed match avoids depending on jq or python.
cwd="$(printf '%s' "${HERDR_PLUGIN_CONTEXT_JSON:-}" |
  sed -n 's/.*"focused_pane_cwd":"\([^"]*\)".*/\1/p')"
if [ -z "$cwd" ] || [ ! -d "$cwd" ]; then
  echo "flowcli: cannot tell the focused pane's directory" >&2
  exit 1
fi

newest=""
for f in "$cwd"/*.mmd; do
  [ -f "$f" ] || continue # the glob stays literal when nothing matches
  if [ -z "$newest" ] || [ "$f" -nt "$newest" ]; then
    newest="$f"
  fi
done
if [ -z "$newest" ]; then
  echo "flowcli: no .mmd file in $cwd" >&2
  exit 1
fi

exec "$herdr" plugin pane open --plugin flowcli --entrypoint view \
  --placement split --direction right --env "FLOWCLI_FILE=$newest"
