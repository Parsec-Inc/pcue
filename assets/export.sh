#!/usr/bin/env bash
# Reexporta todo assets/sprites/*.aseprite para NAME.png (strip horizontal) e NAME.json (json-array).
set -euo pipefail
cd "$(dirname "$0")/sprites"
for f in *.aseprite; do
  n="${f%.aseprite}"
  aseprite -b "$f" --sheet "$n.png" --sheet-type horizontal --data "$n.json" --format json-array
  echo "exported $n"
done
