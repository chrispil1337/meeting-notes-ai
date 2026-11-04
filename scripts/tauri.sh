#!/usr/bin/env bash
# Debug and `tauri dev` builds are Meeting Notes AI Nightly (com.meetingnotesai.desktop.nightly)
# so they coexist with the installed app: separate TCC rows, Dock name, and
# Application Support folder. Release builds keep com.meetingnotesai.desktop.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cfg="$root/app/tauri.nightly.conf.json"

nightly=0
for arg in "$@"; do
  if [[ "$arg" == "dev" || "$arg" == "--debug" ]]; then
    nightly=1
    break
  fi
done

if [[ "$nightly" -eq 1 ]]; then
  exec npx tauri "$@" --config "$cfg"
fi
exec npx tauri "$@"
