#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"
for s in template status-bar-nano status-bar-stock welcome tab-bar-ribbons; do
  cargo run --quiet -- "examples/scripts/${s}.yaml" --out examples/out
done
