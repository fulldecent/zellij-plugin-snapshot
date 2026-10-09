use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use zellij_utils::data::{
    Event, InputMode, ModeInfo, PaneManifest, PermissionStatus, SessionInfo, SessionListSnapshot,
    Styling, TabInfo,
};
use zellij_utils::input::actions::Action;
use zellij_utils::input::config::Config;
use zellij_utils::shared::default_palette;

#[derive(Debug, Deserialize)]
pub struct ShotScript {
    pub name: Option<String>,
    pub plugin: PathBuf,
    #[serde(default)]
    pub config: BTreeMap<String, String>,
    #[serde(default)]
    pub ids: HostIds,
    #[serde(default)]
    pub world: World,
    /// Default `render` size. Required when a render omits `rows` or `cols`, including the implicit render of a script that has no `render` step.
    #[serde(default)]
    pub geometry: Option<Geometry>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Default, Deserialize)]
pub struct World {
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub sessions: Vec<String>,
    #[serde(default)]
    pub resurrectable: Vec<String>,
}

impl World {
    pub fn snapshot(&self) -> SessionListSnapshot {
        let current = self
            .current
            .clone()
            .or_else(|| self.sessions.first().cloned())
            .unwrap_or_else(|| "demo".into());
        let names = if self.sessions.is_empty() {
            vec![current.clone()]
        } else {
            self.sessions.clone()
        };
        SessionListSnapshot {
            live_sessions: names
                .into_iter()
                .map(|name| session_info(&name, name == current))
                .collect(),
            resurrectable_sessions: self
                .resurrectable
                .iter()
                .map(|n| (n.clone(), Duration::from_secs(3600)))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HostIds {
    #[serde(default = "one_u32")]
    pub plugin_id: u32,
    #[serde(default = "one_u32")]
    pub zellij_pid: u32,
    #[serde(default = "one_u16")]
    pub client_id: u16,
    #[serde(default = "tmp_cwd")]
    pub initial_cwd: PathBuf,
}

impl Default for HostIds {
    fn default() -> Self {
        Self {
            plugin_id: 1,
            zellij_pid: 1,
            client_id: 1,
            initial_cwd: PathBuf::from("/tmp"),
        }
    }
}

fn one_u32() -> u32 {
    1
}
fn one_u16() -> u16 {
    1
}
fn tmp_cwd() -> PathBuf {
    PathBuf::from("/tmp")
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Geometry {
    pub rows: u32,
    pub cols: u32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Step {
    GrantPermissions,
    InitialKeybinds,
    ModeUpdate {
        #[serde(default = "normal_mode")]
        mode: String,
        #[serde(default)]
        session: Option<String>,
    },
    TabUpdate {
        tabs: Vec<TabSpec>,
    },
    SessionUpdate {
        current: String,
        #[serde(default)]
        sessions: Vec<String>,
        #[serde(default)]
        resurrectable: Vec<String>,
    },
    /// Inject a fully specified Event as JSON (zellij_utils::data::Event).
    EventJson {
        json: String,
    },
    /// Call `render(rows, cols)` and write `{name}.ansi.txt` and `{name}.svg`.
    ///
    /// This is not an event. Omitted `rows` or `cols` use [`ShotScript::geometry`].
    /// Omitted `name` uses the script stem. A script that contains any `render`
    /// step does not also render once at the end.
    Render {
        #[serde(default)]
        rows: Option<u32>,
        #[serde(default)]
        cols: Option<u32>,
        #[serde(default)]
        name: Option<String>,
    },
}

/// One step of a run, after stems and sizes have been resolved.
#[derive(Debug)]
pub enum Scheduled {
    // `Event` is hundreds of bytes. Boxing keeps the render variant from padding every step.
    Event(Box<Event>),
    Render { stem: String, rows: u32, cols: u32 },
}

fn normal_mode() -> String {
    "normal".into()
}

#[derive(Debug, Deserialize)]
pub struct TabSpec {
    #[serde(default = "tab_name")]
    pub name: String,
    #[serde(default = "true_bool")]
    pub active: bool,
    #[serde(default = "one_usize")]
    pub tiled: usize,
    #[serde(default)]
    pub floating: usize,
    #[serde(default)]
    pub floating_visible: bool,
}

fn tab_name() -> String {
    "Tab".into()
}
fn true_bool() -> bool {
    true
}
fn one_usize() -> usize {
    1
}

impl ShotScript {
    /// Host theme applied on ModeUpdate and when expanding DCS in the rasterizer.
    ///
    /// Matches a default Zellij session: `Styling::from(default_palette())`,
    /// not the `DEFAULT_STYLES` constant (that one paints gray-on-gray bars).
    pub fn styling(&self) -> Styling {
        Styling::from(default_palette())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read script {}", path.display()))?;
        serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Events and renders in script order.
    ///
    /// `default_stem` is the top-level `name`, or the YAML file stem when `name` is omitted.
    /// A script with no `render` step gets one render at the end, using `geometry` and `default_stem`.
    pub fn schedule(&self, default_stem: &str) -> Result<Vec<Scheduled>> {
        let explicit = self.steps.iter().any(|step| step.is_render());
        let mut scheduled = Vec::new();
        let mut stems = Vec::new();
        if explicit {
            for step in &self.steps {
                match step {
                    Step::Render { rows, cols, name } => {
                        let (rows, cols) = resolve_size(*rows, *cols, self.geometry)?;
                        let stem = name.clone().unwrap_or_else(|| default_stem.to_string());
                        check_stem(&stem)?;
                        stems.push(stem.clone());
                        scheduled.push(Scheduled::Render { stem, rows, cols });
                    }
                    other => {
                        if let Some(event) = other.to_event()? {
                            scheduled.push(Scheduled::Event(Box::new(event)));
                        }
                    }
                }
            }
        } else {
            let Some(geometry) = self.geometry else {
                bail!("script has no geometry; set geometry or add a render step");
            };
            for step in &self.steps {
                if let Some(event) = step.to_event()? {
                    scheduled.push(Scheduled::Event(Box::new(event)));
                }
            }
            check_stem(default_stem)?;
            stems.push(default_stem.to_string());
            scheduled.push(Scheduled::Render {
                stem: default_stem.to_string(),
                rows: geometry.rows,
                cols: geometry.cols,
            });
        }
        check_unique(&stems)?;
        Ok(scheduled)
    }
}

fn resolve_size(
    rows: Option<u32>,
    cols: Option<u32>,
    geometry: Option<Geometry>,
) -> Result<(u32, u32)> {
    let rows = rows.or_else(|| geometry.map(|geometry| geometry.rows));
    let cols = cols.or_else(|| geometry.map(|geometry| geometry.cols));
    match (rows, cols) {
        (Some(rows), Some(cols)) => Ok((rows, cols)),
        (None, None) => {
            bail!("render is missing rows and cols; set them on the step or set geometry")
        }
        (None, Some(_)) => {
            bail!("render is missing rows; set rows on the step or set geometry")
        }
        (Some(_), None) => {
            bail!("render is missing cols; set cols on the step or set geometry")
        }
    }
}

fn check_stem(stem: &str) -> Result<()> {
    if stem.is_empty() || stem == "." || stem == ".." || stem.contains('/') || stem.contains('\\') {
        bail!("output stem '{stem}' is not a single file name");
    }
    Ok(())
}

fn check_unique(stems: &[String]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for stem in stems {
        if !seen.insert(stem.as_str()) {
            bail!("duplicate output stem '{stem}'");
        }
    }
    Ok(())
}

impl Step {
    fn is_render(&self) -> bool {
        matches!(self, Step::Render { .. })
    }

    pub fn to_event(&self) -> Result<Option<Event>> {
        match self {
            Step::Render { .. } => Ok(None),
            Step::GrantPermissions => Ok(Some(Event::PermissionRequestResult(
                PermissionStatus::Granted,
            ))),
            Step::InitialKeybinds => Ok(Some(Event::InitialKeybinds(default_keybinds()))),
            Step::ModeUpdate { mode, session } => {
                let mut info = ModeInfo {
                    mode: parse_mode(mode)?,
                    base_mode: Some(InputMode::Normal),
                    session_name: session.clone(),
                    keybinds: default_keybinds(),
                    ..Default::default()
                };
                info.style.colors = Styling::from(default_palette());
                // Zellij sets this false when the session can draw Nerd Font
                // separators. The type default is true, which makes the stock
                // status-bar use empty separators and alternate tiles that
                // paint ribbon_unselected.base on itself (black on black).
                info.capabilities.arrow_fonts = false;
                Ok(Some(Event::ModeUpdate(info)))
            }
            Step::TabUpdate { tabs } => {
                let tabs = tabs
                    .iter()
                    .enumerate()
                    .map(|(i, t)| tab_info(i, t))
                    .collect();
                Ok(Some(Event::TabUpdate(tabs)))
            }
            Step::SessionUpdate {
                current,
                sessions,
                resurrectable,
            } => {
                let world = World {
                    current: Some(current.clone()),
                    sessions: sessions.clone(),
                    resurrectable: resurrectable.clone(),
                };
                let snap = world.snapshot();
                Ok(Some(Event::SessionUpdate(
                    snap.live_sessions,
                    snap.resurrectable_sessions,
                )))
            }
            Step::EventJson { json } => {
                let ev: Event = serde_json::from_str(json).context("event_json")?;
                Ok(Some(ev))
            }
        }
    }
}

fn parse_mode(s: &str) -> Result<InputMode> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "normal" => InputMode::Normal,
        "locked" => InputMode::Locked,
        "pane" => InputMode::Pane,
        "tab" => InputMode::Tab,
        "resize" => InputMode::Resize,
        "move" => InputMode::Move,
        "scroll" => InputMode::Scroll,
        "session" => InputMode::Session,
        other => anyhow::bail!("unknown input mode {other}"),
    })
}

fn session_info(name: &str, is_current: bool) -> SessionInfo {
    SessionInfo {
        name: name.to_string(),
        tabs: vec![tab_info(
            0,
            &TabSpec {
                name: "Tab".into(),
                active: true,
                tiled: 1,
                floating: 0,
                floating_visible: false,
            },
        )],
        panes: PaneManifest::default(),
        connected_clients: 1,
        is_current_session: is_current,
        available_layouts: vec![],
        plugins: BTreeMap::new(),
        web_clients_allowed: false,
        web_client_count: 0,
        tab_history: BTreeMap::new(),
        pane_history: BTreeMap::new(),
        creation_time: Duration::from_secs(0),
    }
}

fn default_keybinds() -> zellij_utils::data::KeybindsVec {
    Config::from_default_assets()
        .map(|c| c.keybinds.to_keybinds_vec())
        .unwrap_or_else(|_| {
            vec![(
                InputMode::Normal,
                vec![(
                    "Ctrl g".parse().expect("key"),
                    vec![Action::SwitchToMode {
                        input_mode: InputMode::Locked,
                    }],
                )],
            )]
        })
}

fn tab_info(position: usize, t: &TabSpec) -> TabInfo {
    TabInfo {
        position,
        name: t.name.clone(),
        active: t.active,
        panes_to_hide: 0,
        is_fullscreen_active: false,
        is_sync_panes_active: false,
        are_floating_panes_visible: t.floating_visible,
        other_focused_clients: vec![],
        active_swap_layout_name: None,
        is_swap_layout_dirty: false,
        viewport_rows: 40,
        viewport_columns: 80,
        display_area_rows: 42,
        display_area_columns: 80,
        selectable_tiled_panes_count: t.tiled,
        selectable_floating_panes_count: t.floating,
        tab_id: position,
        has_bell_notification: false,
        is_flashing_bell: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> ShotScript {
        serde_yaml::from_str(yaml).expect("yaml")
    }

    fn renders(scheduled: &[Scheduled]) -> Vec<(&str, u32, u32)> {
        scheduled
            .iter()
            .filter_map(|step| match step {
                Scheduled::Render { stem, rows, cols } => Some((stem.as_str(), *rows, *cols)),
                Scheduled::Event(_) => None,
            })
            .collect()
    }

    fn kinds(scheduled: &[Scheduled]) -> Vec<&'static str> {
        scheduled
            .iter()
            .map(|step| match step {
                Scheduled::Event(_) => "event",
                Scheduled::Render { .. } => "render",
            })
            .collect()
    }

    #[test]
    fn no_render_step_renders_once_at_the_end() {
        let shot = parse(
            "
            name: demo
            plugin: plugin.wasm
            geometry:
              rows: 1
              cols: 80
            steps:
              - action: grant_permissions
            ",
        );
        let scheduled = shot.schedule("demo").unwrap();
        assert_eq!(kinds(&scheduled), ["event", "render"]);
        assert_eq!(renders(&scheduled), vec![("demo", 1, 80)]);
    }

    #[test]
    fn render_steps_replace_the_trailing_render() {
        let shot = parse(
            "
            plugin: plugin.wasm
            geometry:
              rows: 2
              cols: 80
            steps:
              - action: grant_permissions
              - action: render
                name: narrow
                cols: 40
              - action: grant_permissions
              - action: render
                name: wide
                cols: 120
            ",
        );
        let scheduled = shot.schedule("script").unwrap();
        assert_eq!(kinds(&scheduled), ["event", "render", "event", "render"]);
        assert_eq!(
            renders(&scheduled),
            vec![("narrow", 2, 40), ("wide", 2, 120)]
        );
    }

    #[test]
    fn render_step_can_supply_the_whole_size() {
        let shot = parse(
            "
            plugin: plugin.wasm
            steps:
              - action: render
                name: only
                rows: 3
                cols: 20
            ",
        );
        let scheduled = shot.schedule("script").unwrap();
        assert_eq!(renders(&scheduled), vec![("only", 3, 20)]);
    }

    #[test]
    fn unnamed_render_uses_the_script_stem() {
        let shot = parse(
            "
            plugin: plugin.wasm
            geometry:
              rows: 1
              cols: 80
            steps:
              - action: render
            ",
        );
        let scheduled = shot.schedule("from-file").unwrap();
        assert_eq!(renders(&scheduled), vec![("from-file", 1, 80)]);
    }

    #[test]
    fn duplicate_stems_fail() {
        let shot = parse(
            "
            plugin: plugin.wasm
            geometry:
              rows: 1
              cols: 80
            steps:
              - action: render
              - action: render
                cols: 100
            ",
        );
        let err = shot.schedule("demo").unwrap_err().to_string();
        assert!(err.contains("duplicate output stem 'demo'"), "{err}");
    }

    #[test]
    fn a_stem_is_one_file_name() {
        let shot = parse(
            "
            plugin: plugin.wasm
            steps:
              - action: render
                name: shots/a
                rows: 1
                cols: 1
            ",
        );
        let err = shot.schedule("demo").unwrap_err().to_string();
        assert!(err.contains("not a single file name"), "{err}");
    }

    #[test]
    fn missing_size_fails() {
        let shot = parse(
            "
            plugin: plugin.wasm
            steps:
              - action: render
                name: only
                rows: 1
            ",
        );
        let err = shot.schedule("demo").unwrap_err().to_string();
        assert!(err.contains("missing cols"), "{err}");

        let shot = parse(
            "
            plugin: plugin.wasm
            steps: []
            ",
        );
        let err = shot.schedule("demo").unwrap_err().to_string();
        assert!(err.contains("no geometry"), "{err}");
    }
}
