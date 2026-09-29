#!/usr/bin/env bash
# Launch StashPaper with an empty profile and check that it starts, stays up and
# writes its log, and that a second launch hands over to it and exits.
# Usage: scripts/smoke.sh <path to the stashpaper binary>
# On Linux, run it under `xvfb-run -a dbus-run-session --` for a display and bus.
set -euo pipefail

app=$(realpath "$1")
home=$(mktemp -d)
export HOME="$home"
export XDG_CONFIG_HOME="$home/.config" XDG_DATA_HOME="$home/.local/share" XDG_CACHE_HOME="$home/.cache"

"$app" >"$home/first.out" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true' EXIT
sleep 15

if ! kill -0 "$pid" 2>/dev/null; then
  echo "StashPaper exited during startup:"
  cat "$home/first.out"
  exit 1
fi

log=$(ls "$XDG_DATA_HOME"/com.stashpaper.app/logs/*.log 2>/dev/null | head -1 || true)
if [ -z "$log" ]; then
  echo "No log file under $XDG_DATA_HOME/com.stashpaper.app/logs"
  cat "$home/first.out"
  exit 1
fi
echo "Log file: $log"
cat "$log"
if ! grep -q "StashPaper .* starting" "$log"; then
  echo "The log has no startup line"
  exit 1
fi

# A second copy should hand over to the first (which logs it) and exit
if ! timeout 20 "$app" >"$home/second.out" 2>&1; then
  echo "The second launch didn't exit cleanly:"
  cat "$home/second.out"
  exit 1
fi
sleep 2
if ! kill -0 "$pid" 2>/dev/null; then
  echo "The first copy died when the second one launched"
  exit 1
fi
if ! grep -q "handed over" "$log"; then
  echo "The first copy never heard from the second launch:"
  cat "$log"
  exit 1
fi

# The engine logs instead of crashing the app, so look for that too
if grep -q "\[ERROR\]" "$log"; then
  echo "Errors in the log:"
  grep "\[ERROR\]" "$log"
  exit 1
fi
echo "Smoke test passed"
