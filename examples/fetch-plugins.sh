#!/bin/sh
# Place the four example plugin wasm files under examples/plugins/.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
dest="$root/examples/plugins"
mkdir -p "$dest"

echo ">> template plugin (GitHub release 0.2.0)"
curl -fsSL -o "$dest/fulldecent-zellij-plugin-template-v0.2.0.wasm" \
  "https://github.com/fulldecent/zellij-plugin-template/releases/download/0.2.0/fulldecent-zellij-plugin-template-v0.2.0.wasm"

echo ">> tab-bar-ribbons (sibling zellij-tab-bar-ribbons)"
ribbons_src="$root/../zellij-tab-bar-ribbons"
if [ ! -f "$ribbons_src/Cargo.toml" ]; then
  echo "expected $ribbons_src" >&2
  exit 1
fi
( cd "$ribbons_src" && cargo build --release --target wasm32-wasip1 --bin tab-bar )
cp "$ribbons_src/target/wasm32-wasip1/release/tab-bar.wasm" "$dest/tab-bar-ribbons.wasm"

echo ">> status-bar-nano (sibling zellij-status-bar-ng)"
nano_src="$root/../zellij-status-bar-ng"
if [ ! -f "$nano_src/Cargo.toml" ]; then
  echo "expected $nano_src" >&2
  exit 1
fi
( cd "$nano_src" && cargo build --release --target wasm32-wasip1 --bin status-bar )
cp "$nano_src/target/wasm32-wasip1/release/status-bar.wasm" "$dest/status-bar-nano.wasm"

echo ">> stock status-bar and session-manager (zellij v0.45.1 default-plugins)"
work="$root/.cache/zellij-src"
if [ ! -d "$work/.git" ]; then
  git clone --depth 1 --branch v0.45.1 https://github.com/zellij-org/zellij.git "$work"
fi
( cd "$work" && cargo build --release --target wasm32-wasip1 -p status-bar -p session-manager )
cp "$work/target/wasm32-wasip1/release/status-bar.wasm" "$dest/status-bar-stock.wasm"
cp "$work/target/wasm32-wasip1/release/session-manager.wasm" "$dest/session-manager.wasm"

echo ">> JetBrains Mono Nerd Font"
font_dir="$root/assets/fonts"
mkdir -p "$font_dir"
font="$font_dir/JetBrainsMonoNerdFont-Regular.ttf"
if [ ! -f "$font" ]; then
  curl -fsSL -o "$font" \
    "https://cdn.jsdelivr.net/gh/ryanoasis/nerd-fonts@v3.4.0/patched-fonts/JetBrainsMono/Ligatures/Regular/JetBrainsMonoNerdFont-Regular.ttf"
fi

ls -l "$dest"
echo "ok"
