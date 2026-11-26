#!/usr/bin/env bash
# Starts an installed copy of the desktop app on a virtual display. Passes once
# the web view has loaded the interface and the interface has reached the
# engine, and keeps a screenshot.
#
# Usage: start-check.sh <path-to-app> <name-for-the-output-files>
set -u
app="$1"; name="$2"
out="${RUNNER_TEMP:-/tmp}"
export TUNNEL_DATA_DIR="$out/data-$name" TUNNEL_LOG=info WEBKIT_DISABLE_DMABUF_RENDERER=1
log="$out/start-$name.txt"
xvfb-run -a -s "-screen 0 1280x800x24" bash -c '
  "$0" 2> "$1" &
  app=$!
  for _ in $(seq 45); do
    grep -q "interface ready" "$1" && break
    kill -0 "$app" 2>/dev/null || break
    sleep 2
  done
  sleep 5
  import -window root "$2" || true
  alive=0
  kill -0 "$app" 2>/dev/null && alive=1
  kill "$app" 2>/dev/null
  [ "$alive" -eq 1 ]
' "$app" "$log" "$out/screen-$name.png"
alive=$?
cat "$log"
if [ "$alive" -ne 0 ]; then echo "The app exited on its own"; exit 1; fi
if ! grep -q "interface ready" "$log"; then echo "The interface never reached the engine"; exit 1; fi
echo "The app started and its interface reached the engine."
