#!/usr/bin/env bash
# Regenera os .aseprite a partir dos scripts Lua e reexporta png/json.
set -euo pipefail
src="$(cd "$(dirname "$0")" && pwd)"
out="$(dirname "$src")"
for s in mascot icons; do
  aseprite -b --script-param "dir=$out" --script-param "src=$src" --script "$src/$s.lua"
done
"$(dirname "$out")/export.sh"
