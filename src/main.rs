mod host;
mod raster;
mod script;
mod trace;

use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};

use crate::host::PluginHost;
use crate::script::ShotScript;

#[derive(Parser, Debug)]
#[command(
    name = "zellij-plugin-snapshot",
    about = "Zellij Plugin Snapshot: load a plugin wasm, drive it, capture exact render output"
)]
struct Args {
    /// YAML drive script (plugin path, config, events, geometry).
    /// Relative paths are from the current working directory.
    script: PathBuf,
    /// Directory for `{name}.ansi.txt` and `{name}.svg`
    #[arg(long, default_value = ".")]
    out: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let script_path = abs_cwd(&args.script);
    let mut shot = ShotScript::load(&script_path)?;
    let script_dir = script_path.parent().unwrap_or(Path::new("."));
    shot.plugin = abs(script_dir, &shot.plugin);
    if !shot.plugin.exists() {
        anyhow::bail!("plugin wasm missing: {}", shot.plugin.display());
    }

    let mut host = PluginHost::load(
        &shot.plugin,
        &shot.config,
        &shot.ids,
        shot.world.snapshot(),
    )?;
    let log = host.drive(&shot.steps)?;
    let ansi = host.render(shot.geometry.rows, shot.geometry.cols)?;
    let styling = shot.styling();
    let cols = shot.geometry.cols;
    let rows = shot.geometry.rows;

    let stem = shot
        .name
        .unwrap_or_else(|| script_path.file_stem().unwrap().to_string_lossy().into());
    std::fs::create_dir_all(&args.out)?;
    let ansi_path = args.out.join(format!("{stem}.ansi.txt"));
    let svg_path = args.out.join(format!("{stem}.svg"));
    std::fs::write(&ansi_path, &ansi)?;
    let svg = raster::ansi_svg(&ansi, cols, rows, &styling);
    std::fs::write(&svg_path, svg)?;

    println!("plugin commands during run:");
    for line in &log {
        println!("  {line}");
    }
    println!("wrote {}", ansi_path.display());
    println!("wrote {}", svg_path.display());
    Ok(())
}

fn abs_cwd(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    }
}

fn abs(root: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}
