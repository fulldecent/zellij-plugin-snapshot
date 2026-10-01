use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
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
    pub geometry: Geometry,
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
}

impl Step {
    pub fn to_event(&self) -> Result<Option<Event>> {
        match self {
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
