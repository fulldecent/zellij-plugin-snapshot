# Zellij Plugin Snapshot

A command-line host that loads **your** Zellij plugin `.wasm`, feeds it the same events a real session would, and writes two files:

| File | What it is |
|---|---|
| `{name}.ansi.txt` | Exact bytes from `render(rows, cols)` (SGR + Zellij UI DCS) |
| `{name}.svg` | That pane painted as self-contained glyph outlines (no browser, no font install) |

Pinned to Zellij / `zellij-tile` / `zellij-utils` **0.45.1**. A plugin built against another version will not speak this protobuf.

This crate is a **command**. You run it against a wasm you already built.

Do not list it in `[dependencies]`: that graph is compiled into the plugin you ship.

Do not list it in `[dev-dependencies]` either, on stable Cargo. That field still means “link this **library** into `cargo test`.” This package has no library target, and even if it did, Cargo would not put the snapshot **binary** on your `PATH` or into your tests. Unstable [artifact dependencies](https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#artifact-dependencies) (`artifact = "bin"`) are the Cargo-native way to depend on someone else’s binary; this project does not require that.

Install it from Git (there is no crates.io release):

```bash
cargo install --git https://github.com/fulldecent/zellij-plugin-snapshot --tag v0.1.0 --locked
```

---

## Use it on your plugin

### 1. Build your plugin wasm

From your plugin crate (the one with `crate-type = ["cdylib"]` and `zellij-tile = "0.45.1"`):

```bash
rustup target add wasm32-wasip1
cargo build --target wasm32-wasip1 --release
```

The artifact is typically:

```text
target/wasm32-wasip1/release/<your_crate>.wasm
```

### 2. Get this tool

```bash
cargo install --git https://github.com/fulldecent/zellij-plugin-snapshot --tag v0.1.0 --locked
```

To work from a clone:

```bash
git clone https://github.com/fulldecent/zellij-plugin-snapshot.git
cd zellij-plugin-snapshot
cargo build --release
```

The Nerd Font is compiled into the binary. Font licenses are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

### 3. Write a drive script

A YAML file describes the pane size and the events to send before `render`. Put it in *your* repo. Paths in `plugin:` that are not absolute are **relative to this YAML file**.

`shots/normal.yaml`:

```yaml
name: normal
plugin: ../target/wasm32-wasip1/release/your_plugin.wasm
geometry:
  rows: 1
  cols: 80
steps:
  - action: grant_permissions
  - action: mode_update
    mode: normal
    session: demo
  - action: tab_update
    tabs:
      - name: Tab
        active: true
        tiled: 1
```

`name` is the output stem (`normal.ansi.txt`, `normal.svg`). If omitted, the YAML file stem is used.

### 4. Run the host

From **your** plugin directory:

```bash
# via cargo, without installing
cargo run --manifest-path ../zellij-plugin-snapshot/Cargo.toml --release -- \
  shots/normal.yaml --out shots

# or if installed
zellij-plugin-snapshot shots/normal.yaml --out shots
```

`--out` is the directory for the two files (created if needed). Default is the current working directory.

Commit the YAML and the `.ansi.txt` snapshot. The SVG is for looking at; do not `diff` it in tests.

### 5. Tests and CI

A test or CI job is: build wasm, run the host, `diff` the **ANSI** file.

```bash
cargo build --target wasm32-wasip1 --release
zellij-plugin-snapshot shots/normal.yaml --out /tmp/shots
diff -u shots/normal.ansi.txt /tmp/shots/normal.ansi.txt
```

GitHub Actions sketch:

```yaml
- uses: dtolnay/rust-toolchain@stable
  with:
    targets: wasm32-wasip1
- run: cargo build --target wasm32-wasip1 --release
- run: cargo install --git https://github.com/fulldecent/zellij-plugin-snapshot --tag v0.1.0 --locked
- run: zellij-plugin-snapshot shots/normal.yaml --out /tmp/shots
- run: diff -u shots/normal.ansi.txt /tmp/shots/normal.ansi.txt
```

---

## Drive script reference

```yaml
name: optional-output-stem
plugin: path/to/plugin.wasm          # relative to this YAML file
config:                               # string map passed to load()
  welcome_screen: "true"
ids:                                  # optional; defaults are 1 / /tmp
  plugin_id: 1
  zellij_pid: 1
  client_id: 1
  initial_cwd: /tmp
world:                                # session list for GetSessionList
  current: demo
  sessions: [demo]
  resurrectable: []
geometry:
  rows: 1                             # passed to render(rows, cols)
  cols: 80
steps:                                # events, in order, then render
  - action: grant_permissions
  - action: initial_keybinds
  - action: mode_update
    mode: normal                      # default normal
    session: demo
  - action: tab_update
    tabs:
      - name: Tab
        active: true
        tiled: 1
        floating: 0
        floating_visible: false
  - action: session_update
    current: demo
    sessions: [demo]
    resurrectable: []
  - action: event_json
    json: '{"ModeUpdate": ...}'       # raw zellij_utils::data::Event
```

`mode_update` always applies a default-session theme (`Styling::from(default_palette())`) and `arrow_fonts: false` (Nerd Font separators), matching a typical local Zellij 0.45 session.

**Which steps your plugin needs** depends on what it reads in `update`:

| Plugin kind | Typical steps |
|---|---|
| Ignores host (hello-world) | `steps: []` |
| Status / tab bar | `grant_permissions`, `mode_update`, `tab_update` |
| Stock status-bar | also `initial_keybinds` (it paints the keymap) |
| Session / welcome UI | `world:` plus `session_update` |

If `render` is empty or the wasm traps, add the query the plugin makes (`GetSessionList`, permissions, …) via the fields above. Unknown host commands are stubbed so the module can continue.

---

## Outputs

- **ANSI** (`.ansi.txt`) — source of truth for snapshots. `diff` this file. Open it in a terminal, or `cat` it. Includes private DCS (`ESC Pz … ESC \`) when the plugin uses Zellij ribbons/tables/text.
- **SVG** — the same pane as outlines, for humans. Cell grid is 9.60×21.12 px per column/row at 16px JetBrains Mono Nerd Font. Viewers do not need the font installed. Do not snapshot-`diff` SVG.

Theme for DCS expansion is the same default palette as `mode_update`. Unstyled cells sit on a black pane.

This tool captures **one plugin** per run (`render(rows, cols)` for that wasm). It does not load a Zellij layout or compose tab bar, panes, and status bar into one image.

---

## This repository’s examples

These check the host against known plugins. They are not required to snapshot yours.

```bash
sh examples/fetch-plugins.sh
sh examples/run-all.sh
```

That writes `examples/out/{template,status-bar-nano,status-bar-stock,welcome,tab-bar-ribbons}.{ansi.txt,svg}`.

`fetch-plugins.sh` downloads the fulldecent template wasm, builds sibling `zellij-status-bar-ng` and `zellij-tab-bar-ribbons` if those trees exist, and builds Zellij v0.45.1 `status-bar` and `session-manager`.

---

## CLI

```text
zellij-plugin-snapshot script.yaml [--out DIR]
```

| Argument | Meaning |
|---|---|
| `script.yaml` | Drive script; relative to the current working directory |
| `--out DIR` | Directory for `{name}.ansi.txt` and `{name}.svg` (default `.`) |

## Maintenance and dependency updates

Do this every month or so and please send a PR here if you see updates available:

1. Identify external Actions in [.github/workflows](.github/workflows) scripts and look for available new versions. Review and then update to the new version if it is safe. GitHub-supported Actions (i.e. under the `actions/` organization) may require only cursory review.
1. Review crates in [Cargo.toml](Cargo.toml) and refresh [Cargo.lock](Cargo.lock). `anyhow`, `clap`, `serde`, `serde_json`, `ttf-parser`, and `unicode-width` may take current compatible releases.
1. Keep `zellij-utils` **0.45.1**, `wasmi` / `wasmi_wasi` **1.1.0**, and `prost` **0.12** until Zellij itself ships a newer ABI. Wasmi 2.x is a different interpreter than Zellij 0.45 uses. This snapshot tool will only ever support the latest version of Zellij and `zellij-utils`.
1. `serde_yaml` 0.9 is deprecated. A later swap should be a maintained 0.9-compatible crate such as `serde_yaml_ng`, not `serde_yml`.
1. Review the Zellij tag and sibling plugin paths in [examples/fetch-plugins.sh](examples/fetch-plugins.sh) when example WASMs should track a new host.

## References

1. This crate is a command-line host. Plugin authors run the binary against their wasm. They do not add it to `[dependencies]` or to `[dev-dependencies]` on stable Cargo (that field links a library, and this package is not one).
1. We use title case for titles and proper nouns; not for headings and things. This includes this README as well as workflow rules and other configuration files.
1. We use an MIT license for this project’s source, Copyright (c) 2026 William Entriken. See [LICENSE](LICENSE). The bundled JetBrains Mono Nerd Font is OFL 1.1 plus Nerd Fonts’ terms; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
1. Zellij can load a plugin from an HTTPS URL. That is simpler and insecure. We treat that as wrong and do not document it.
1. This project is built based on [best practices documented in zellij-plugin-template](https://github.com/fulldecent/zellij-plugin-template), release 1.0.0.
1. This project is built based on [best practices documented in project-template](https://github.com/fulldecent/project-template), release 1.0.0.
