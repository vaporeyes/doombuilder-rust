// ABOUTME: Iced application root for doombuilder-rust.
// ABOUTME: UDB-style layout: dynamic title, toolbar with map picker, full
// ABOUTME: viewport, bottom inspector with texture slots, status bar.

mod camera;
mod icons;
mod style;
mod view2d;
mod view3d;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use doombuilder_core::archive::{open as open_asset, Asset, Pk3};
use doombuilder_core::config::GameConfig;
use doombuilder_core::edit::{
    collect_and_delete, compute_make_sector, Command, LineEndpoint, LinedefChain, LinedefIntField,
    SectorIntField, SectorSlot, SidedefSlot, ThingIntField, ThingMove, UndoStack, VertexMove,
};
use doombuilder_core::map::LinedefId;
use doombuilder_core::map::MapThing;
use doombuilder_core::map::{
    save_map_as_pwad, Map, MapLinedef, MapSidedef, MapVertex, SectorId, TextureName, ThingId,
    VertexId,
};
use doombuilder_core::textures::TextureSet;
use doombuilder_core::wad::WadKind;
use doombuilder_core::{load_auto, MapFormat, Wad};
use doombuilder_render::{
    build_walls, extract_sector_loops, rasterise_sector_fill, triangulate_sector, FloorMesh,
    SpatialIndex, Wall,
};
use glam::Vec2;
use iced::keyboard::{self, Modifiers};
use iced::widget::canvas::Cache;
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{
    button, checkbox, column, container, image, mouse_area, pick_list, row, scrollable, stack,
    text, text_input, Space,
};
use iced::{Color, Element, Length, Subscription, Task, Theme};

use camera::Camera2D;
use view2d::{map_aabb, FillTile, HighlightKind, View2D, View2DMessage};
use view3d::{build_geometry, world_aabb, Camera3D, View3D, View3DGeometry, View3DMessage};

pub fn run() -> iced::Result {
    iced::application(App::default, App::update, App::view)
        .title(App::window_title)
        .subscription(App::subscription)
        .theme(App::theme)
        .run()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    View2D,
    View3D,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    Vertices,
    Linedefs,
    Sectors,
    Things,
}

impl Default for EditMode {
    fn default() -> Self {
        EditMode::Linedefs
    }
}

impl EditMode {
    pub fn label(self) -> &'static str {
        match self {
            EditMode::Vertices => "Vertices",
            EditMode::Linedefs => "Linedefs",
            EditMode::Sectors => "Sectors",
            EditMode::Things => "Things",
        }
    }
}

pub struct App {
    status: String,
    wad: Option<Wad>,
    wad_path: Option<PathBuf>,
    summary: Option<String>,
    maps: Vec<String>,
    selected_map: Option<String>,
    map: Option<Arc<Map>>,
    map_stats: Option<MapStats>,
    sector_meshes: Arc<Vec<(SectorId, FloorMesh)>>,
    walls: Arc<Vec<Wall>>,
    spatial: Option<Arc<SpatialIndex>>,
    sector_fills: Arc<Vec<FillTile>>,
    settings: Settings,
    camera2d: Camera2D,
    camera3d: Camera3D,
    geometry3d: Arc<View3DGeometry>,
    cache2d: Arc<Cache>,
    hover: Option<HighlightKind>,
    selection: Arc<HashSet<HighlightKind>>,
    drag_rect: Option<(Vec2, Vec2)>,
    active_drag: Option<DragMode>,
    cursor_world: Option<Vec2>,
    undo: UndoStack,
    modifiers: Modifiers,
    mode: Mode,
    edit_mode: EditMode,
    config: Arc<GameConfig>,
    current_config_name: String,
    textures: Option<Arc<TextureSet>>,
    texture_handles: Arc<HashMap<String, ImageHandle>>,
    sprite_handles: Arc<HashMap<String, ImageHandle>>,
    sprite_dims: Arc<HashMap<String, (u32, u32)>>,
    sorted_texture_names: Arc<Vec<String>>,
    active_picker: Option<ActivePicker>,
    picker_filter: String,
    sector_buffers: Option<SectorBuffers>,
    linedef_buffers: Option<LinedefBuffers>,
    thing_buffers: Option<ThingBuffers>,
    drawing: Option<DrawingState>,
}

#[derive(Debug, Default)]
pub struct DrawingState {
    pub chain: LinedefChain,
    /// Current chain head (live VertexId in the map + how we'd serialise it).
    pub last: Option<(VertexId, LineEndpoint)>,
}

#[derive(Debug, Clone)]
struct SectorBuffers {
    sector: SectorId,
    floor: String,
    ceiling: String,
    light: String,
    tag: String,
}

#[derive(Debug, Clone)]
struct LinedefBuffers {
    line: doombuilder_core::map::LinedefId,
    tag: String,
    args: [String; 5],
}

#[derive(Debug, Clone)]
struct ThingBuffers {
    thing: ThingId,
    angle: String,
}

#[derive(Debug, Clone, Copy)]
pub enum ActivePicker {
    Texture(PickerTarget),
    Action(doombuilder_core::map::LinedefId),
    ThingKind(ThingId),
    SectorSpecial(SectorId),
    Settings,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    #[serde(default = "default_true")]
    pub show_textures: bool,
    #[serde(default = "default_true")]
    pub show_sprites: bool,
    #[serde(default = "default_true")]
    pub show_grid: bool,
    #[serde(default = "default_true")]
    pub show_things: bool,
    #[serde(default)]
    pub always_show_vertices: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_textures: true,
            show_sprites: true,
            show_grid: true,
            show_things: true,
            always_show_vertices: false,
        }
    }
}

impl Settings {
    /// Path to the JSON settings file: e.g. `~/.config/doombuilder/settings.json`
    /// on Linux, `~/Library/Application Support/doombuilder/settings.json` on
    /// macOS, `%APPDATA%\doombuilder\settings.json` on Windows.
    pub fn config_path() -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "doombuilder")?;
        Some(dirs.config_dir().join("settings.json"))
    }

    pub fn load_or_default() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = Self::config_path() else {
            return Err("no settings directory available".into());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SettingKey {
    ShowTextures,
    ShowSprites,
    ShowGrid,
    ShowThings,
    AlwaysShowVertices,
}

impl SettingKey {
    fn label(self) -> &'static str {
        match self {
            SettingKey::ShowTextures => "Show sector textures (flats)",
            SettingKey::ShowSprites => "Show thing sprites (vs colored placeholders)",
            SettingKey::ShowGrid => "Show grid",
            SettingKey::ShowThings => "Show things",
            SettingKey::AlwaysShowVertices => "Always show vertex dots",
        }
    }

    fn get(self, s: &Settings) -> bool {
        match self {
            SettingKey::ShowTextures => s.show_textures,
            SettingKey::ShowSprites => s.show_sprites,
            SettingKey::ShowGrid => s.show_grid,
            SettingKey::ShowThings => s.show_things,
            SettingKey::AlwaysShowVertices => s.always_show_vertices,
        }
    }

    fn set(self, s: &mut Settings, v: bool) {
        match self {
            SettingKey::ShowTextures => s.show_textures = v,
            SettingKey::ShowSprites => s.show_sprites = v,
            SettingKey::ShowGrid => s.show_grid = v,
            SettingKey::ShowThings => s.show_things = v,
            SettingKey::AlwaysShowVertices => s.always_show_vertices = v,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PickerTarget {
    Sidedef {
        sidedef: doombuilder_core::map::SidedefId,
        slot: SidedefSlot,
    },
    Sector {
        sector: SectorId,
        slot: SectorSlot,
    },
}


#[derive(Debug)]
enum DragMode {
    Rect,
    MoveVertices {
        /// Original (x, y) for each vertex involved in the drag.
        originals: Vec<(VertexId, i32, i32)>,
    },
    MoveThings {
        originals: Vec<(ThingId, i32, i32)>,
    },
}

impl Default for App {
    fn default() -> Self {
        Self {
            status: String::new(),
            wad: None,
            wad_path: None,
            summary: None,
            maps: Vec::new(),
            selected_map: None,
            map: None,
            map_stats: None,
            sector_meshes: Arc::new(Vec::new()),
            walls: Arc::new(Vec::new()),
            spatial: None,
            sector_fills: Arc::new(Vec::new()),
            settings: Settings::load_or_default(),
            camera2d: Camera2D::default(),
            camera3d: Camera3D::default(),
            geometry3d: Arc::new(View3DGeometry::default()),
            cache2d: Arc::new(Cache::new()),
            hover: None,
            selection: Arc::new(HashSet::new()),
            drag_rect: None,
            active_drag: None,
            cursor_world: None,
            undo: UndoStack::new(),
            modifiers: Modifiers::default(),
            mode: Mode::default(),
            edit_mode: EditMode::default(),
            config: Arc::new(GameConfig::vanilla_doom()),
            current_config_name: "Doom".to_string(),
            textures: None,
            texture_handles: Arc::new(HashMap::new()),
            sprite_handles: Arc::new(HashMap::new()),
            sprite_dims: Arc::new(HashMap::new()),
            sorted_texture_names: Arc::new(Vec::new()),
            active_picker: None,
            picker_filter: String::new(),
            sector_buffers: None,
            linedef_buffers: None,
            thing_buffers: None,
            drawing: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    OpenWadRequested,
    SaveMapRequested,
    SaveMapPathPicked(Option<PathBuf>),
    SaveMapDone(Result<PathBuf, String>),
    FilePicked(Option<PathBuf>),
    AssetLoaded(Result<AssetSummary, String>),
    MapSelected(String),
    MapLoaded(Result<MapPayload, String>),
    Mode(Mode),
    SetEditMode(EditMode),
    ToggleTextures,
    OpenSettings,
    SetSetting(SettingKey, bool),
    SetGameConfig(String),
    View2D(View2DMessage),
    View3D(View3DMessage),
    ModifiersChanged(Modifiers),
    KeyboardEsc,
    SelectAll,
    Undo,
    Redo,
    DeleteSelection,
    InsertThing,
    ToggleDrawing,
    CancelDrawing,
    MakeSector,
    OpenTexturePicker(PickerTarget),
    OpenActionPicker(doombuilder_core::map::LinedefId),
    OpenThingKindPicker(ThingId),
    OpenSectorSpecialPicker(SectorId),
    ClosePicker,
    PickTexture(String),
    PickAction(u16),
    PickThingKind(u16),
    PickSectorSpecial(u16),
    PickerFilterChanged(String),
    ToggleLinedefFlag { id: doombuilder_core::map::LinedefId, bit: u16 },
    ToggleThingFlag { id: ThingId, bit: u16 },
    SectorFieldChanged { field: SectorIntField, text: String },
    SectorFieldSubmit(SectorIntField),
    LinedefFieldChanged { field: LinedefIntField, text: String },
    LinedefFieldSubmit(LinedefIntField),
    ThingFieldChanged { field: ThingIntField, text: String },
    ThingFieldSubmit(ThingIntField),
    Quit,
    Noop,
}

#[derive(Debug, Clone)]
pub struct AssetSummary {
    path: PathBuf,
    wad: Option<Wad>,
    textures: Option<Arc<TextureSet>>,
    texture_handles: Arc<HashMap<String, ImageHandle>>,
    sprite_handles: Arc<HashMap<String, ImageHandle>>,
    sprite_dims: Arc<HashMap<String, (u32, u32)>>,
    summary: String,
    maps: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MapStats {
    name: String,
    format: MapFormat,
    vertices: usize,
    linedefs: usize,
    sidedefs: usize,
    sectors: usize,
    things: usize,
}

#[derive(Debug, Clone)]
pub struct MapPayload {
    map: Arc<Map>,
    sector_meshes: Arc<Vec<(SectorId, FloorMesh)>>,
    walls: Arc<Vec<Wall>>,
    spatial: Arc<SpatialIndex>,
    stats: MapStats,
}

impl App {
    fn theme(&self) -> Theme {
        Theme::Light
    }

    fn persist_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.status = format!("Settings save failed: {e}");
        }
    }

    fn window_title(&self) -> String {
        let mut t = String::from("DoomBuilder");
        if let Some(path) = self.wad_path.as_ref().and_then(|p| p.file_name()) {
            t.push_str(" — ");
            t.push_str(&path.to_string_lossy());
        }
        if let Some(name) = &self.selected_map {
            t.push_str(" (");
            t.push_str(name);
            t.push(')');
        }
        t
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle_message(message);
        self.refresh_inspector_buffers();
        task
    }

    fn refresh_inspector_buffers(&mut self) {
        let single = if self.selection.len() == 1 {
            self.selection.iter().next().copied()
        } else {
            None
        };
        let map = self.map.as_ref();

        // Sector
        let want_sector = match single {
            Some(HighlightKind::Sector(id)) => Some(id),
            _ => None,
        };
        match (want_sector, &self.sector_buffers) {
            (Some(id), Some(b)) if b.sector == id => {}
            (Some(id), _) => {
                if let Some(s) = map.and_then(|m| m.sectors.get(id)) {
                    self.sector_buffers = Some(SectorBuffers {
                        sector: id,
                        floor: s.floor_height.to_string(),
                        ceiling: s.ceiling_height.to_string(),
                        light: s.light.to_string(),
                        tag: s.tag.to_string(),
                    });
                }
            }
            (None, _) => self.sector_buffers = None,
        }

        // Linedef
        let want_line = match single {
            Some(HighlightKind::Linedef(id)) => Some(id),
            _ => None,
        };
        match (want_line, &self.linedef_buffers) {
            (Some(id), Some(b)) if b.line == id => {}
            (Some(id), _) => {
                if let Some(l) = map.and_then(|m| m.linedefs.get(id)) {
                    self.linedef_buffers = Some(LinedefBuffers {
                        line: id,
                        tag: l.tag.to_string(),
                        args: [
                            l.args[0].to_string(),
                            l.args[1].to_string(),
                            l.args[2].to_string(),
                            l.args[3].to_string(),
                            l.args[4].to_string(),
                        ],
                    });
                }
            }
            (None, _) => self.linedef_buffers = None,
        }

        // Thing
        let want_thing = match single {
            Some(HighlightKind::Thing(id)) => Some(id),
            _ => None,
        };
        match (want_thing, &self.thing_buffers) {
            (Some(id), Some(b)) if b.thing == id => {}
            (Some(id), _) => {
                if let Some(t) = map.and_then(|m| m.things.get(id)) {
                    self.thing_buffers = Some(ThingBuffers {
                        thing: id,
                        angle: t.angle.to_string(),
                    });
                }
            }
            (None, _) => self.thing_buffers = None,
        }
    }

    fn handle_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenWadRequested => {
                self.status = "Choose a WAD, PK3, or zip...".to_string();
                Task::perform(pick_file(), Message::FilePicked)
            }
            Message::SaveMapRequested => {
                if self.map.is_none() {
                    self.status = "Open a map before saving.".into();
                    return Task::none();
                }
                let suggested = self
                    .selected_map
                    .clone()
                    .unwrap_or_else(|| "MAP".to_string());
                Task::perform(pick_save_path(suggested), Message::SaveMapPathPicked)
            }
            Message::SaveMapPathPicked(None) => {
                self.status = "Save cancelled.".into();
                Task::none()
            }
            Message::SaveMapPathPicked(Some(path)) => {
                let Some(map) = self.map.clone() else {
                    self.status = "No map to save.".into();
                    return Task::none();
                };
                self.status = format!("Saving {}...", path.display());
                Task::perform(save_map_to_path(map, path), Message::SaveMapDone)
            }
            Message::SaveMapDone(Ok(path)) => {
                self.status = format!("Saved {}", path.display());
                Task::none()
            }
            Message::SaveMapDone(Err(err)) => {
                self.status = format!("Save failed: {err}");
                Task::none()
            }
            Message::FilePicked(None) => {
                self.status = "Open cancelled.".to_string();
                Task::none()
            }
            Message::FilePicked(Some(path)) => {
                self.status = format!("Loading {}...", path.display());
                Task::perform(load_asset(path), Message::AssetLoaded)
            }
            Message::AssetLoaded(Ok(asset)) => {
                self.status = format!("Loaded {}", asset.path.display());
                self.wad_path = Some(asset.path);
                self.wad = asset.wad;
                self.textures = asset.textures;
                let mut sorted: Vec<String> =
                    asset.texture_handles.keys().cloned().collect();
                sorted.sort();
                self.sorted_texture_names = Arc::new(sorted);
                self.texture_handles = asset.texture_handles;
                self.sprite_handles = asset.sprite_handles;
                self.sprite_dims = asset.sprite_dims;
                self.summary = Some(asset.summary);
                self.maps = asset.maps;
                self.reset_map_state();
                Task::none()
            }
            Message::AssetLoaded(Err(err)) => {
                self.status = format!("Load failed: {err}");
                Task::none()
            }
            Message::MapSelected(name) => {
                let Some(wad) = self.wad.clone() else {
                    self.status = "Map loading is only supported for WADs right now.".into();
                    return Task::none();
                };
                self.status = format!("Loading {name}...");
                self.selected_map = Some(name.clone());
                Task::perform(load_map_payload(wad, name), Message::MapLoaded)
            }
            Message::MapLoaded(Ok(payload)) => {
                self.status = format!("Loaded {}", payload.stats.name);
                self.map_stats = Some(payload.stats);
                self.map = Some(payload.map.clone());
                self.sector_meshes = payload.sector_meshes;
                self.walls = payload.walls;
                self.spatial = Some(payload.spatial);
                self.cache2d = Arc::new(Cache::new());
                self.hover = None;
                self.selection = Arc::new(HashSet::new());
                self.drag_rect = None;
                if let Some((min, max)) = map_aabb(&payload.map) {
                    self.camera2d.frame_aabb(min, max, Vec2::new(800.0, 600.0));
                }
                self.rebuild_sector_fills();
                self.rebuild_geometry3d();
                if let Some((min, max)) = world_aabb(self.map.as_ref().unwrap(), &self.sector_meshes) {
                    self.camera3d.frame_aabb(min, max);
                }
                Task::none()
            }
            Message::MapLoaded(Err(err)) => {
                self.status = format!("Map load failed: {err}");
                Task::none()
            }
            Message::Mode(mode) => {
                self.mode = mode;
                Task::none()
            }
            Message::SetEditMode(em) => {
                if self.edit_mode != em {
                    self.edit_mode = em;
                    // Drop selection elements that aren't compatible with the
                    // new mode, then drop the hover too.
                    let mut sel = (*self.selection).clone();
                    sel.retain(|h| edit_mode_matches(em, *h));
                    self.selection = Arc::new(sel);
                    self.hover = None;
                    self.cache2d.clear();
                }
                Task::none()
            }
            Message::ToggleTextures => {
                self.settings.show_textures = !self.settings.show_textures;
                self.persist_settings();
                self.cache2d.clear();
                Task::none()
            }
            Message::OpenSettings => {
                self.active_picker = Some(ActivePicker::Settings);
                Task::none()
            }
            Message::SetSetting(key, value) => {
                key.set(&mut self.settings, value);
                self.persist_settings();
                self.cache2d.clear();
                Task::none()
            }
            Message::SetGameConfig(name) => {
                if let Some(cfg) = GameConfig::builtin(&name) {
                    self.config = Arc::new(cfg);
                    self.current_config_name = name;
                    // Rebuild 3D thing colors / categories etc. since they
                    // sample from the active config.
                    self.rebuild_geometry_indices();
                    self.cache2d.clear();
                }
                Task::none()
            }
            Message::View2D(msg) => {
                self.handle_view2d(msg);
                self.cache2d.clear();
                Task::none()
            }
            Message::View3D(msg) => {
                self.handle_view3d(msg);
                Task::none()
            }
            Message::ModifiersChanged(m) => {
                self.modifiers = m;
                Task::none()
            }
            Message::KeyboardEsc => {
                if self.drawing.is_some() {
                    self.cancel_drawing();
                } else if self.active_drag.is_some() {
                    self.cancel_active_drag();
                } else if !self.selection.is_empty() {
                    self.selection = Arc::new(HashSet::new());
                }
                self.cache2d.clear();
                Task::none()
            }
            Message::Undo => {
                if let Some(map) = self.map.as_mut() {
                    if self.undo.undo(Arc::make_mut(map)) {
                        self.rebuild_geometry_indices();
                        self.cache2d.clear();
                    }
                }
                Task::none()
            }
            Message::Redo => {
                if let Some(map) = self.map.as_mut() {
                    if self.undo.redo(Arc::make_mut(map)) {
                        self.rebuild_geometry_indices();
                        self.cache2d.clear();
                    }
                }
                Task::none()
            }
            Message::SelectAll => {
                if let Some(map) = &self.map {
                    let all: HashSet<HighlightKind> = match self.edit_mode {
                        EditMode::Vertices => {
                            map.vertices.keys().map(HighlightKind::Vertex).collect()
                        }
                        EditMode::Linedefs => {
                            map.linedefs.keys().map(HighlightKind::Linedef).collect()
                        }
                        EditMode::Sectors => map.sectors.keys().map(HighlightKind::Sector).collect(),
                        EditMode::Things => map.things.keys().map(HighlightKind::Thing).collect(),
                    };
                    self.selection = Arc::new(all);
                    self.cache2d.clear();
                }
                Task::none()
            }
            Message::OpenTexturePicker(target) => {
                self.active_picker = Some(ActivePicker::Texture(target));
                self.picker_filter.clear();
                Task::none()
            }
            Message::OpenActionPicker(id) => {
                self.active_picker = Some(ActivePicker::Action(id));
                self.picker_filter.clear();
                Task::none()
            }
            Message::OpenThingKindPicker(id) => {
                self.active_picker = Some(ActivePicker::ThingKind(id));
                self.picker_filter.clear();
                Task::none()
            }
            Message::OpenSectorSpecialPicker(id) => {
                self.active_picker = Some(ActivePicker::SectorSpecial(id));
                self.picker_filter.clear();
                Task::none()
            }
            Message::ClosePicker => {
                self.active_picker = None;
                self.picker_filter.clear();
                Task::none()
            }
            Message::PickerFilterChanged(q) => {
                self.picker_filter = q;
                Task::none()
            }
            Message::PickAction(new_special) => {
                if let Some(ActivePicker::Action(id)) = self.active_picker.take() {
                    if let Some(map) = self.map.as_mut() {
                        let map_mut = Arc::make_mut(map);
                        if let Some(line) = map_mut.linedefs.get(id) {
                            let old = line.special;
                            if old != new_special {
                                let mut cmd = Command::SetLinedefSpecial {
                                    id,
                                    old,
                                    new: new_special,
                                };
                                cmd.apply(map_mut);
                                self.undo.push(cmd);
                                self.cache2d.clear();
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::PickSectorSpecial(new_special) => {
                if let Some(ActivePicker::SectorSpecial(id)) = self.active_picker.take() {
                    if let Some(map) = self.map.as_mut() {
                        let map_mut = Arc::make_mut(map);
                        if let Some(s) = map_mut.sectors.get(id) {
                            let old = s.special as i32;
                            let new = new_special as i32;
                            if old != new {
                                let mut cmd = Command::SetSectorIntField {
                                    id,
                                    field: SectorIntField::Special,
                                    old,
                                    new,
                                };
                                cmd.apply(map_mut);
                                self.undo.push(cmd);
                                self.cache2d.clear();
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::ToggleLinedefFlag { id, bit } => {
                if let Some(map) = self.map.as_mut() {
                    let map_mut = Arc::make_mut(map);
                    if let Some(line) = map_mut.linedefs.get(id) {
                        let old = line.flags as i32;
                        let new = (line.flags ^ bit) as i32;
                        let mut cmd = Command::SetLinedefIntField {
                            id,
                            field: LinedefIntField::Flags,
                            old,
                            new,
                        };
                        cmd.apply(map_mut);
                        self.undo.push(cmd);
                        self.cache2d.clear();
                    }
                }
                Task::none()
            }
            Message::ToggleThingFlag { id, bit } => {
                if let Some(map) = self.map.as_mut() {
                    let map_mut = Arc::make_mut(map);
                    if let Some(t) = map_mut.things.get(id) {
                        let old = t.flags as i32;
                        let new = (t.flags ^ bit) as i32;
                        let mut cmd = Command::SetThingIntField {
                            id,
                            field: ThingIntField::Flags,
                            old,
                            new,
                        };
                        cmd.apply(map_mut);
                        self.undo.push(cmd);
                        self.cache2d.clear();
                    }
                }
                Task::none()
            }
            Message::PickThingKind(new_kind) => {
                if let Some(ActivePicker::ThingKind(id)) = self.active_picker.take() {
                    if let Some(map) = self.map.as_mut() {
                        let map_mut = Arc::make_mut(map);
                        if let Some(t) = map_mut.things.get(id) {
                            let old = t.kind;
                            if old != new_kind {
                                let mut cmd = Command::SetThingKind {
                                    id,
                                    old,
                                    new: new_kind,
                                };
                                cmd.apply(map_mut);
                                self.undo.push(cmd);
                                self.rebuild_geometry_indices();
                                self.cache2d.clear();
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::PickTexture(name) => {
                if let Some(ActivePicker::Texture(target)) = self.active_picker.take() {
                    if let Some(map) = self.map.as_mut() {
                        let map_mut = Arc::make_mut(map);
                        let mut padded = [0u8; 8];
                        let bytes = name.as_bytes();
                        let len = bytes.len().min(8);
                        padded[..len].copy_from_slice(&bytes[..len]);
                        let new = TextureName(padded);

                        let cmd = match target {
                            PickerTarget::Sidedef { sidedef, slot } => map_mut
                                .sidedefs
                                .get(sidedef)
                                .map(|s| {
                                    let old = match slot {
                                        SidedefSlot::Upper => s.upper_texture,
                                        SidedefSlot::Middle => s.middle_texture,
                                        SidedefSlot::Lower => s.lower_texture,
                                    };
                                    Command::SetSidedefTexture {
                                        id: sidedef,
                                        slot,
                                        old,
                                        new,
                                    }
                                }),
                            PickerTarget::Sector { sector, slot } => map_mut
                                .sectors
                                .get(sector)
                                .map(|s| {
                                    let old = match slot {
                                        SectorSlot::Floor => s.floor_texture,
                                        SectorSlot::Ceiling => s.ceiling_texture,
                                    };
                                    Command::SetSectorTexture {
                                        id: sector,
                                        slot,
                                        old,
                                        new,
                                    }
                                }),
                        };
                        if let Some(mut cmd) = cmd {
                            cmd.apply(map_mut);
                            self.undo.push(cmd);
                            self.rebuild_geometry_indices();
                            self.cache2d.clear();
                        }
                    }
                }
                Task::none()
            }
            Message::DeleteSelection => {
                let mut sel_v: HashSet<doombuilder_core::map::VertexId> = HashSet::new();
                let mut sel_l: HashSet<LinedefId> = HashSet::new();
                let mut sel_sec: HashSet<SectorId> = HashSet::new();
                let mut sel_t: HashSet<ThingId> = HashSet::new();
                for h in self.selection.iter() {
                    match h {
                        HighlightKind::Vertex(v) => {
                            sel_v.insert(*v);
                        }
                        HighlightKind::Linedef(l) => {
                            sel_l.insert(*l);
                        }
                        HighlightKind::Sector(s) => {
                            sel_sec.insert(*s);
                        }
                        HighlightKind::Thing(t) => {
                            sel_t.insert(*t);
                        }
                    }
                }
                let any = !sel_v.is_empty()
                    || !sel_l.is_empty()
                    || !sel_sec.is_empty()
                    || !sel_t.is_empty();
                if any {
                    if let Some(map) = self.map.as_mut() {
                        let map_mut = Arc::make_mut(map);
                        let state = collect_and_delete(map_mut, &sel_v, &sel_l, &sel_sec, &sel_t);
                        let nothing = state.vertex_snaps.is_empty()
                            && state.sector_snaps.is_empty()
                            && state.sidedef_snaps.is_empty()
                            && state.linedef_snaps.is_empty()
                            && state.thing_snaps.is_empty();
                        if !nothing {
                            self.undo
                                .push(Command::DeleteElements(Box::new(state)));
                            self.selection = Arc::new(HashSet::new());
                            self.rebuild_geometry_indices();
                            self.cache2d.clear();
                        }
                    }
                }
                Task::none()
            }
            Message::InsertThing => {
                let world = self.cursor_world.unwrap_or(self.camera2d.center);
                if let Some(map) = self.map.as_mut() {
                    let map_mut = Arc::make_mut(map);
                    let snapshot = MapThing {
                        x: world.x.round() as i32,
                        y: world.y.round() as i32,
                        angle: 0,
                        kind: 1,
                        flags: 7,
                        tid: 0,
                        z: 0,
                        special: 0,
                        args: [0; 5],
                    };
                    let id = map_mut.things.insert(snapshot.clone());
                    self.undo.push(Command::CreateThing {
                        id: Some(id),
                        snapshot,
                    });
                    let mut sel = HashSet::new();
                    sel.insert(HighlightKind::Thing(id));
                    self.selection = Arc::new(sel);
                    self.rebuild_geometry_indices();
                    self.cache2d.clear();
                }
                Task::none()
            }
            Message::SectorFieldChanged { field, text } => {
                if let Some(b) = self.sector_buffers.as_mut() {
                    if let Some(slot) = sector_buffer_field_mut(b, field) {
                        slot.clear();
                        slot.push_str(&text);
                    }
                }
                Task::none()
            }
            Message::SectorFieldSubmit(field) => {
                let Some(b) = self.sector_buffers.clone() else {
                    return Task::none();
                };
                let parsed: Option<i32> = sector_buffer_field(&b, field).trim().parse().ok();
                if let (Some(new), Some(map)) = (parsed, self.map.as_mut()) {
                    let map_mut = Arc::make_mut(map);
                    if let Some(sec) = map_mut.sectors.get(b.sector) {
                        let old = match field {
                            SectorIntField::FloorHeight => sec.floor_height as i32,
                            SectorIntField::CeilingHeight => sec.ceiling_height as i32,
                            SectorIntField::Light => sec.light as i32,
                            SectorIntField::Tag => sec.tag as i32,
                            SectorIntField::Special => sec.special as i32,
                        };
                        if old != new {
                            let mut cmd = Command::SetSectorIntField {
                                id: b.sector,
                                field,
                                old,
                                new,
                            };
                            cmd.apply(map_mut);
                            self.undo.push(cmd);
                            // Heights affect 3D geometry; light affects 3D shading;
                            // either way rebuild keeps both views consistent.
                            self.rebuild_geometry_indices();
                            self.cache2d.clear();
                        }
                    }
                }
                Task::none()
            }
            Message::LinedefFieldChanged { field, text } => {
                if let Some(b) = self.linedef_buffers.as_mut() {
                    if let Some(slot) = linedef_buffer_field_mut(b, field) {
                        slot.clear();
                        slot.push_str(&text);
                    }
                }
                Task::none()
            }
            Message::LinedefFieldSubmit(field) => {
                let Some(b) = self.linedef_buffers.clone() else {
                    return Task::none();
                };
                let parsed: Option<i32> = linedef_buffer_field(&b, field).trim().parse().ok();
                if let (Some(new), Some(map)) = (parsed, self.map.as_mut()) {
                    let map_mut = Arc::make_mut(map);
                    if let Some(line) = map_mut.linedefs.get(b.line) {
                        let old = match field {
                            LinedefIntField::Flags => line.flags as i32,
                            LinedefIntField::Tag => line.tag as i32,
                            LinedefIntField::Arg0 => line.args[0] as i32,
                            LinedefIntField::Arg1 => line.args[1] as i32,
                            LinedefIntField::Arg2 => line.args[2] as i32,
                            LinedefIntField::Arg3 => line.args[3] as i32,
                            LinedefIntField::Arg4 => line.args[4] as i32,
                        };
                        if old != new {
                            let mut cmd = Command::SetLinedefIntField {
                                id: b.line,
                                field,
                                old,
                                new,
                            };
                            cmd.apply(map_mut);
                            self.undo.push(cmd);
                            self.cache2d.clear();
                        }
                    }
                }
                Task::none()
            }
            Message::ThingFieldChanged { field, text } => {
                if let Some(b) = self.thing_buffers.as_mut() {
                    if let Some(slot) = thing_buffer_field_mut(b, field) {
                        slot.clear();
                        slot.push_str(&text);
                    }
                }
                Task::none()
            }
            Message::ThingFieldSubmit(field) => {
                let Some(b) = self.thing_buffers.clone() else {
                    return Task::none();
                };
                let parsed: Option<i32> = thing_buffer_field(&b, field).trim().parse().ok();
                if let (Some(new), Some(map)) = (parsed, self.map.as_mut()) {
                    let map_mut = Arc::make_mut(map);
                    if let Some(t) = map_mut.things.get(b.thing) {
                        let old = match field {
                            ThingIntField::Angle => t.angle as i32,
                            ThingIntField::Flags => t.flags as i32,
                        };
                        if old != new {
                            let mut cmd = Command::SetThingIntField {
                                id: b.thing,
                                field,
                                old,
                                new,
                            };
                            cmd.apply(map_mut);
                            self.undo.push(cmd);
                            // Angle changes affect 2D arrow direction; cache only.
                            self.cache2d.clear();
                        }
                    }
                }
                Task::none()
            }
            Message::ToggleDrawing => {
                if self.drawing.is_some() {
                    // Pressing toggle again commits the chain.
                    self.commit_drawing();
                } else if self.map.is_some() {
                    self.drawing = Some(DrawingState::default());
                    self.status = "Drawing: click to chain linedefs (Esc cancels, D commits)".into();
                }
                self.cache2d.clear();
                Task::none()
            }
            Message::CancelDrawing => {
                self.cancel_drawing();
                self.cache2d.clear();
                Task::none()
            }
            Message::MakeSector => {
                self.do_make_sector();
                Task::none()
            }
            Message::Quit => iced::exit(),
            Message::Noop => Task::none(),
        }
    }

    fn reset_map_state(&mut self) {
        self.selected_map = None;
        self.map = None;
        self.map_stats = None;
        self.sector_meshes = Arc::new(Vec::new());
        self.walls = Arc::new(Vec::new());
        self.spatial = None;
        self.sector_fills = Arc::new(Vec::new());
        self.cache2d = Arc::new(Cache::new());
        self.hover = None;
        self.selection = Arc::new(HashSet::new());
        self.drag_rect = None;
        self.active_drag = None;
        self.undo.clear();
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().map(|event| match event {
            keyboard::Event::ModifiersChanged(m) => Message::ModifiersChanged(m),
            keyboard::Event::KeyPressed { key, modifiers, .. } => match key.as_ref() {
                keyboard::Key::Named(keyboard::key::Named::Escape) => Message::KeyboardEsc,
                keyboard::Key::Character("a") if modifiers.command() => Message::SelectAll,
                keyboard::Key::Character("z") if modifiers.command() && modifiers.shift() => {
                    Message::Redo
                }
                keyboard::Key::Character("z") if modifiers.command() => Message::Undo,
                keyboard::Key::Character("y") if modifiers.command() => Message::Redo,
                keyboard::Key::Character("s") if modifiers.command() => Message::SaveMapRequested,
                keyboard::Key::Named(keyboard::key::Named::Delete)
                | keyboard::Key::Named(keyboard::key::Named::Backspace) => {
                    Message::DeleteSelection
                }
                keyboard::Key::Named(keyboard::key::Named::Insert) => Message::InsertThing,
                keyboard::Key::Character("i") if !modifiers.command() => Message::InsertThing,
                keyboard::Key::Character("d") if !modifiers.command() => Message::ToggleDrawing,
                keyboard::Key::Character("m") if modifiers.command() => Message::MakeSector,
                keyboard::Key::Character("1") if !modifiers.command() => {
                    Message::SetEditMode(EditMode::Vertices)
                }
                keyboard::Key::Character("2") if !modifiers.command() => {
                    Message::SetEditMode(EditMode::Linedefs)
                }
                keyboard::Key::Character("3") if !modifiers.command() => {
                    Message::SetEditMode(EditMode::Sectors)
                }
                keyboard::Key::Character("4") if !modifiers.command() => {
                    Message::SetEditMode(EditMode::Things)
                }
                _ => Message::ModifiersChanged(modifiers),
            },
            keyboard::Event::KeyReleased { modifiers, .. } => {
                Message::ModifiersChanged(modifiers)
            }
        })
    }

    fn handle_view3d(&mut self, msg: View3DMessage) {
        match msg {
            View3DMessage::OrbitBy { dx, dy } => {
                self.camera3d.yaw -= dx * 0.005;
                self.camera3d.pitch =
                    (self.camera3d.pitch - dy * 0.005).clamp(0.05, std::f32::consts::FRAC_PI_2 - 0.05);
            }
            View3DMessage::Zoom { factor } => {
                self.camera3d.distance =
                    (self.camera3d.distance * factor).clamp(64.0, 50_000.0);
            }
        }
    }

    fn rebuild_geometry3d(&mut self) {
        let (Some(map), Some(textures)) = (&self.map, &self.textures) else {
            self.geometry3d = Arc::new(View3DGeometry::default());
            return;
        };
        let geom = build_geometry(
            map,
            &self.sector_meshes,
            &self.walls,
            textures,
            self.spatial.as_deref(),
            &self.config,
        );
        self.geometry3d = Arc::new(geom);
    }

    fn handle_view2d(&mut self, msg: View2DMessage) {
        match msg {
            View2DMessage::PanBy(delta) => self.camera2d.pan_screen(delta),
            View2DMessage::ZoomAt {
                pivot,
                factor,
                viewport,
            } => self.camera2d.zoom_about(pivot, viewport, factor),
            View2DMessage::HoverAt(world) => {
                self.cursor_world = Some(world);
                let new_hover = self.hit_test(world);
                if new_hover != self.hover {
                    self.hover = new_hover;
                }
            }
            View2DMessage::HoverCleared => {
                self.hover = None;
                self.cursor_world = None;
            }
            View2DMessage::ClickAt(world) => {
                if self.drawing.is_some() {
                    self.drawing_click(world);
                } else {
                    let hit = self.hit_test(world);
                    let additive = self.modifiers.shift();
                    let mut sel: HashSet<HighlightKind> = (*self.selection).clone();
                    match (hit, additive) {
                        (Some(h), true) => {
                            if !sel.insert(h) {
                                sel.remove(&h);
                            }
                        }
                        (Some(h), false) => {
                            sel.clear();
                            sel.insert(h);
                        }
                        (None, false) => sel.clear(),
                        (None, true) => {}
                    }
                    self.selection = Arc::new(sel);
                }
            }
            View2DMessage::DragMoved { start, current } => {
                self.handle_drag_moved(start, current);
            }
            View2DMessage::DragComplete { start, end } => {
                self.handle_drag_complete(start, end);
            }
        }
    }

    fn handle_drag_moved(&mut self, start: Vec2, current: Vec2) {
        // First DragMoved decides the drag mode based on what the press hit.
        if self.active_drag.is_none() {
            let hit = self.hit_test(start);
            self.active_drag = Some(self.begin_drag(hit, start));
        }
        let mode = self.active_drag.as_ref();
        match mode {
            Some(DragMode::Rect) => {
                self.drag_rect = Some((start, current));
                self.hover = None;
            }
            Some(DragMode::MoveVertices { originals }) => {
                self.hover = None;
                let dx = (current.x - start.x).round() as i32;
                let dy = (current.y - start.y).round() as i32;
                let originals = originals.clone();
                if let Some(map) = self.map.as_mut() {
                    let map = Arc::make_mut(map);
                    for &(id, ox, oy) in &originals {
                        if let Some(v) = map.vertices.get_mut(id) {
                            v.x = ox.saturating_add(dx);
                            v.y = oy.saturating_add(dy);
                        }
                    }
                }
                // Drop sector fills until drag ends so they don't appear stale.
                self.sector_meshes = Arc::new(Vec::new());
            }
            Some(DragMode::MoveThings { originals }) => {
                self.hover = None;
                let dx = (current.x - start.x).round() as i32;
                let dy = (current.y - start.y).round() as i32;
                let originals = originals.clone();
                if let Some(map) = self.map.as_mut() {
                    let map = Arc::make_mut(map);
                    for &(id, ox, oy) in &originals {
                        if let Some(t) = map.things.get_mut(id) {
                            t.x = ox.saturating_add(dx);
                            t.y = oy.saturating_add(dy);
                        }
                    }
                }
            }
            None => {}
        }
    }

    fn handle_drag_complete(&mut self, start: Vec2, end: Vec2) {
        let mode = self.active_drag.take();
        match mode {
            Some(DragMode::Rect) => {
                self.drag_rect = None;
                if let (Some(spatial), Some(map)) = (&self.spatial, self.map.as_ref()) {
                    let min = [start.x.min(end.x), start.y.min(end.y)];
                    let max = [start.x.max(end.x), start.y.max(end.y)];
                    let mut sel: HashSet<HighlightKind> = if self.modifiers.shift() {
                        (*self.selection).clone()
                    } else {
                        HashSet::new()
                    };
                    match self.edit_mode {
                        EditMode::Vertices => {
                            for v in spatial.vertices_in_rect(min, max) {
                                sel.insert(HighlightKind::Vertex(v));
                            }
                        }
                        EditMode::Linedefs => {
                            for l in spatial.linedefs_in_rect(min, max) {
                                sel.insert(HighlightKind::Linedef(l));
                            }
                        }
                        EditMode::Things => {
                            for (id, t) in &map.things {
                                let tx = t.x as f32;
                                let ty = t.y as f32;
                                if tx >= min[0] && tx <= max[0] && ty >= min[1] && ty <= max[1] {
                                    sel.insert(HighlightKind::Thing(id));
                                }
                            }
                        }
                        EditMode::Sectors => {
                            // No bulk rect-select for sectors; UDB matches.
                        }
                    }
                    self.selection = Arc::new(sel);
                }
            }
            Some(DragMode::MoveVertices { originals }) => {
                let dx = (end.x - start.x).round() as i32;
                let dy = (end.y - start.y).round() as i32;
                if dx != 0 || dy != 0 {
                    let moves: Vec<VertexMove> = originals
                        .iter()
                        .map(|&(id, _, _)| VertexMove { id, dx, dy })
                        .collect();
                    self.undo.push(Command::MoveVertices(moves));
                } else {
                    if let Some(map) = self.map.as_mut() {
                        let map = Arc::make_mut(map);
                        for &(id, ox, oy) in &originals {
                            if let Some(v) = map.vertices.get_mut(id) {
                                v.x = ox;
                                v.y = oy;
                            }
                        }
                    }
                }
                self.rebuild_geometry_indices();
            }
            Some(DragMode::MoveThings { originals }) => {
                let dx = (end.x - start.x).round() as i32;
                let dy = (end.y - start.y).round() as i32;
                if dx != 0 || dy != 0 {
                    let moves: Vec<ThingMove> = originals
                        .iter()
                        .map(|&(id, _, _)| ThingMove { id, dx, dy })
                        .collect();
                    self.undo.push(Command::MoveThings(moves));
                } else if let Some(map) = self.map.as_mut() {
                    let map = Arc::make_mut(map);
                    for &(id, ox, oy) in &originals {
                        if let Some(t) = map.things.get_mut(id) {
                            t.x = ox;
                            t.y = oy;
                        }
                    }
                }
                self.rebuild_geometry_indices();
            }
            None => {}
        }
    }

    fn begin_drag(&mut self, hit: Option<HighlightKind>, _start: Vec2) -> DragMode {
        match hit {
            Some(h @ HighlightKind::Thing(_)) => {
                // Promote to selection (replace or shift-add) before dragging.
                if self.modifiers.shift() {
                    let mut sel = (*self.selection).clone();
                    sel.insert(h);
                    self.selection = Arc::new(sel);
                } else if !self.selection.contains(&h) {
                    let mut sel = HashSet::new();
                    sel.insert(h);
                    self.selection = Arc::new(sel);
                }
                let originals = self.collect_drag_things();
                if !originals.is_empty() {
                    return DragMode::MoveThings { originals };
                }
                DragMode::Rect
            }
            Some(h @ (HighlightKind::Vertex(_) | HighlightKind::Linedef(_))) => {
                if self.modifiers.shift() {
                    let mut sel = (*self.selection).clone();
                    sel.insert(h);
                    self.selection = Arc::new(sel);
                } else if !self.selection.contains(&h) {
                    let mut sel = HashSet::new();
                    sel.insert(h);
                    self.selection = Arc::new(sel);
                }
                let originals = self.collect_drag_vertices();
                if !originals.is_empty() {
                    return DragMode::MoveVertices { originals };
                }
                DragMode::Rect
            }
            _ => DragMode::Rect,
        }
    }

    fn collect_drag_things(&self) -> Vec<(ThingId, i32, i32)> {
        let map = match &self.map {
            Some(m) => m,
            None => return Vec::new(),
        };
        self.selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Thing(id) => map.things.get(*id).map(|t| (*id, t.x, t.y)),
                _ => None,
            })
            .collect()
    }

    /// Vertices that should move with the current selection (vertex selections
    /// contribute themselves; linedef selections contribute both endpoints).
    fn collect_drag_vertices(&self) -> Vec<(VertexId, i32, i32)> {
        let map = match &self.map {
            Some(m) => m,
            None => return Vec::new(),
        };
        let mut ids: HashSet<VertexId> = HashSet::new();
        for h in self.selection.iter() {
            match h {
                HighlightKind::Vertex(v) => {
                    ids.insert(*v);
                }
                HighlightKind::Linedef(l) => {
                    if let Some(line) = map.linedefs.get(*l) {
                        ids.insert(line.v1);
                        ids.insert(line.v2);
                    }
                }
                HighlightKind::Sector(_) | HighlightKind::Thing(_) => {}
            }
        }
        ids.into_iter()
            .filter_map(|id| map.vertices.get(id).map(|v| (id, v.x, v.y)))
            .collect()
    }

    fn do_make_sector(&mut self) {
        let line_ids: Vec<doombuilder_core::map::LinedefId> = self
            .selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Linedef(id) => Some(*id),
                _ => None,
            })
            .collect();
        if line_ids.is_empty() {
            self.status = "Make Sector: select a closed loop of linedefs first.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else {
            return;
        };
        match compute_make_sector(map, &line_ids) {
            Ok(state) => {
                let mut cmd = Command::MakeSector(Box::new(state));
                if let Some(map) = self.map.as_mut() {
                    let map_mut = Arc::make_mut(map);
                    cmd.apply(map_mut);
                    self.undo.push(cmd);
                    self.rebuild_geometry_indices();
                    self.cache2d.clear();
                    self.status = format!("Made sector from {} linedefs.", line_ids.len());
                }
            }
            Err(doombuilder_core::edit::MakeSectorError::NoLines) => {
                self.status = "Make Sector: no linedefs selected.".into();
            }
            Err(doombuilder_core::edit::MakeSectorError::LineHasSides) => {
                self.status =
                    "Make Sector: at least one selected linedef already has a sidedef.".into();
            }
            Err(doombuilder_core::edit::MakeSectorError::NotAClosedLoop) => {
                self.status = "Make Sector: selected lines do not form a single closed loop.".into();
            }
            Err(doombuilder_core::edit::MakeSectorError::DanglingVertex) => {
                self.status =
                    "Make Sector: each vertex in the loop must touch exactly two selected lines.".into();
            }
        }
    }

    fn drawing_click(&mut self, world: Vec2) {
        let Some(drawing) = self.drawing.as_mut() else {
            return;
        };
        let Some(map) = self.map.as_mut() else {
            return;
        };
        let map_mut = Arc::make_mut(map);

        // Snap to nearest existing vertex within ~8 px (world units / zoom).
        let snap_world = (8.0_f32 / self.camera2d.zoom.max(1e-6)).max(2.0);
        let snapped = self
            .spatial
            .as_ref()
            .and_then(|sp| sp.nearest_vertex(world.x, world.y, snap_world));

        let (target_vid, target_endpoint) = match snapped {
            Some(vid) => (vid, LineEndpoint::Existing(vid)),
            None => {
                let vsnap = MapVertex {
                    x: world.x.round() as i32,
                    y: world.y.round() as i32,
                };
                let new_vid = map_mut.vertices.insert(vsnap);
                drawing.chain.vertex_inserts.push(vsnap);
                drawing.chain.current_v.push(new_vid);
                let idx = drawing.chain.vertex_inserts.len() - 1;
                (new_vid, LineEndpoint::New(idx))
            }
        };

        if let Some((from_vid, from_endpoint)) = drawing.last.clone() {
            if from_vid != target_vid {
                let template = MapLinedef {
                    v1: from_vid,
                    v2: target_vid,
                    flags: 0,
                    special: 0,
                    args: [0; 5],
                    tag: 0,
                    right: None,
                    left: None,
                };
                let new_lid = map_mut.linedefs.insert(template.clone());
                drawing.chain.current_l.push(new_lid);
                drawing.chain.linedefs.push((
                    from_endpoint,
                    target_endpoint.clone(),
                    template,
                ));
            }
        }
        drawing.last = Some((target_vid, target_endpoint));
        self.status = format!(
            "Drawing: {} verts, {} lines (Esc cancels, D commits)",
            drawing.chain.current_v.len(),
            drawing.chain.current_l.len()
        );
        // Spatial index is now stale; rebuild on commit/cancel rather than per-click.
        self.cache2d.clear();
    }

    fn cancel_drawing(&mut self) {
        let Some(drawing) = self.drawing.take() else {
            return;
        };
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            for id in drawing.chain.current_l.iter().rev() {
                map_mut.linedefs.remove(*id);
            }
            for id in drawing.chain.current_v.iter().rev() {
                map_mut.vertices.remove(*id);
            }
        }
        self.rebuild_geometry_indices();
        self.status = "Drawing cancelled.".into();
    }

    fn commit_drawing(&mut self) {
        let Some(drawing) = self.drawing.take() else {
            return;
        };
        if drawing.chain.linedefs.is_empty() && drawing.chain.vertex_inserts.is_empty() {
            self.status = "Drawing committed (empty).".into();
            return;
        }
        let count_v = drawing.chain.current_v.len();
        let count_l = drawing.chain.current_l.len();
        let new_lines: Vec<doombuilder_core::map::LinedefId> = drawing.chain.current_l.clone();
        self.undo
            .push(Command::CreateLinedefChain(Box::new(drawing.chain)));
        self.rebuild_geometry_indices();
        // Auto-select the freshly-drawn linedefs and switch to Linedefs mode
        // so the user can flow straight into Make Sector.
        let mut sel = HashSet::new();
        for id in &new_lines {
            sel.insert(HighlightKind::Linedef(*id));
        }
        self.selection = Arc::new(sel);
        self.edit_mode = EditMode::Linedefs;
        self.cache2d.clear();
        self.status = format!(
            "Drew {} vertices and {} linedefs (selected for Make Sector).",
            count_v, count_l
        );
    }

    fn cancel_active_drag(&mut self) {
        let mode = self.active_drag.take();
        match mode {
            Some(DragMode::MoveVertices { originals }) => {
                if let Some(map) = self.map.as_mut() {
                    let map = Arc::make_mut(map);
                    for &(id, ox, oy) in &originals {
                        if let Some(v) = map.vertices.get_mut(id) {
                            v.x = ox;
                            v.y = oy;
                        }
                    }
                }
                self.rebuild_geometry_indices();
            }
            Some(DragMode::MoveThings { originals }) => {
                if let Some(map) = self.map.as_mut() {
                    let map = Arc::make_mut(map);
                    for &(id, ox, oy) in &originals {
                        if let Some(t) = map.things.get_mut(id) {
                            t.x = ox;
                            t.y = oy;
                        }
                    }
                }
                self.rebuild_geometry_indices();
            }
            _ => {}
        }
        self.drag_rect = None;
    }

    fn rebuild_geometry_indices(&mut self) {
        let Some(map) = self.map.clone() else { return };
        let loops = doombuilder_render::extract_sector_loops(&map);
        let mut meshes_with_id: Vec<(SectorId, FloorMesh)> = Vec::with_capacity(loops.len());
        for (sid, lp) in &loops {
            if let Ok(mesh) = doombuilder_render::triangulate_sector(&map, *sid, lp) {
                if !mesh.indices.is_empty() {
                    meshes_with_id.push((*sid, mesh));
                }
            }
        }
        let walls = build_walls(&map);
        let spatial = SpatialIndex::build(&map, meshes_with_id.clone());
        self.sector_meshes = Arc::new(meshes_with_id);
        self.walls = Arc::new(walls);
        self.spatial = Some(Arc::new(spatial));
        self.rebuild_sector_fills();
        self.rebuild_geometry3d();
    }

    fn rebuild_sector_fills(&mut self) {
        let (Some(map), Some(textures)) = (&self.map, &self.textures) else {
            self.sector_fills = Arc::new(Vec::new());
            return;
        };
        let mut tiles: Vec<FillTile> = Vec::new();
        for (sid, mesh) in self.sector_meshes.iter() {
            let Some(fill) = rasterise_sector_fill(map, *sid, mesh, textures) else {
                continue;
            };
            if fill.width == 0 || fill.height == 0 {
                continue;
            }
            let handle = ImageHandle::from_rgba(fill.width, fill.height, fill.rgba);
            let world_min = Vec2::new(fill.origin_world.0, fill.origin_world.1 - fill.height as f32);
            let world_max = Vec2::new(
                fill.origin_world.0 + fill.width as f32,
                fill.origin_world.1,
            );
            tiles.push(FillTile {
                handle,
                world_min,
                world_max,
            });
        }
        self.sector_fills = Arc::new(tiles);
    }

    fn hit_test(&self, world: Vec2) -> Option<HighlightKind> {
        let spatial = self.spatial.as_ref()?;
        let zoom = self.camera2d.zoom.max(1e-6);
        let vertex_radius = 8.0 / zoom;
        let linedef_radius = 5.0 / zoom;
        match self.edit_mode {
            EditMode::Vertices => spatial
                .nearest_vertex(world.x, world.y, vertex_radius)
                .map(HighlightKind::Vertex),
            EditMode::Linedefs => spatial
                .nearest_linedef(world.x, world.y, linedef_radius)
                .map(HighlightKind::Linedef),
            EditMode::Sectors => spatial.sector_at(world.x, world.y).map(HighlightKind::Sector),
            EditMode::Things => spatial
                .nearest_thing(world.x, world.y, 24.0)
                .map(HighlightKind::Thing),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let menu = self.menu_bar();
        let toolbar = self.toolbar();
        let viewport = self.viewport_widget();
        let mut layout = column![menu, toolbar, viewport].spacing(0);
        if let Some(panel) = self.bottom_panel() {
            layout = layout.push(panel);
        }
        layout = layout.push(self.status_bar());
        let main: Element<'_, Message> = layout.into();
        if self.active_picker.is_some() {
            stack![main, self.picker_modal()].into()
        } else {
            main
        }
    }

    fn menu_bar(&self) -> Element<'_, Message> {
        container(
            row![
                menu_picker("File", FILE_MENU_ITEMS, dispatch_file),
                menu_picker("Edit", EDIT_MENU_ITEMS, dispatch_edit),
                menu_picker("View", VIEW_MENU_ITEMS, dispatch_view),
                menu_picker("Tools", TOOLS_MENU_ITEMS, dispatch_tools),
                menu_picker("Help", HELP_MENU_ITEMS, dispatch_help),
            ]
            .spacing(2)
            .padding(2)
            .align_y(iced::Alignment::Center),
        )
        .style(menu_bar_style)
        .width(Length::Fill)
        .into()
    }

    fn toolbar(&self) -> Element<'_, Message> {
        let map_picker: Element<'_, Message> = if self.maps.is_empty() {
            text("No map").size(13).into()
        } else {
            pick_list(
                self.maps.clone(),
                self.selected_map.clone(),
                Message::MapSelected,
            )
            .placeholder("Pick map…")
            .into()
        };

        let toolbar_row = row![
            icons::icon_cmd_btn(icons::FOLDER_OPEN, "Open WAD\u{2026}", Message::OpenWadRequested),
            icons::icon_cmd_btn(icons::SAVE_DISK, "Save Map As\u{2026}", Message::SaveMapRequested),
            vertical_separator(),
            text("Map:").size(13),
            map_picker,
            vertical_separator(),
            icons::icon_btn(icons::VIEW_2D, "2D View", Message::Mode(Mode::View2D), self.mode == Mode::View2D),
            icons::icon_btn(icons::VIEW_3D, "3D View", Message::Mode(Mode::View3D), self.mode == Mode::View3D),
            vertical_separator(),
            text("Game:").size(13),
            pick_list(
                GameConfig::builtin_names()
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
                Some(self.current_config_name.clone()),
                Message::SetGameConfig,
            ),
            vertical_separator(),
            icons::icon_btn(icons::VERTEX, "Vertices mode (1)", Message::SetEditMode(EditMode::Vertices), self.edit_mode == EditMode::Vertices),
            icons::icon_btn(icons::LINEDEF, "Linedefs mode (2)", Message::SetEditMode(EditMode::Linedefs), self.edit_mode == EditMode::Linedefs),
            icons::icon_btn(icons::SECTOR, "Sectors mode (3)", Message::SetEditMode(EditMode::Sectors), self.edit_mode == EditMode::Sectors),
            icons::icon_btn(icons::THING, "Things mode (4)", Message::SetEditMode(EditMode::Things), self.edit_mode == EditMode::Things),
            vertical_separator(),
            icons::icon_btn(icons::DRAW_PEN, "Draw lines (D)", Message::ToggleDrawing, self.drawing.is_some()),
            icons::icon_cmd_btn(icons::MAKE_SECTOR, "Make sector from selected lines (\u{2318}M)", Message::MakeSector),
            icons::icon_btn(icons::TEXTURES, "Show sector textures", Message::ToggleTextures, self.settings.show_textures),
            icons::icon_cmd_btn(icons::SETTINGS_GEAR, "Settings\u{2026}", Message::OpenSettings),
        ]
        .spacing(4)
        .padding(6)
        .align_y(iced::Alignment::Center);

        container(
            scrollable(toolbar_row)
                .direction(iced::widget::scrollable::Direction::Horizontal(
                    iced::widget::scrollable::Scrollbar::new()
                        .width(4)
                        .scroller_width(4),
                ))
                .width(Length::Fill),
        )
        .style(panel_style)
        .width(Length::Fill)
        .into()
    }

    fn viewport_widget(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = match (self.mode, &self.map) {
            (_, None) => container(text("Open a WAD and pick a map to begin.").size(16))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into(),
            (Mode::View2D, Some(map)) => {
                let view = View2D {
                    map: map.clone(),
                    meshes: self.sector_meshes.clone(),
                    camera: self.camera2d,
                    cache: self.cache2d.clone(),
                    hover: self.hover,
                    selection: self.selection.clone(),
                    drag_rect: self.drag_rect,
                    fills: if self.settings.show_textures {
                        self.sector_fills.clone()
                    } else {
                        Arc::new(Vec::new())
                    },
                    config: self.config.clone(),
                    edit_mode: self.edit_mode,
                    sprite_handles: self.sprite_handles.clone(),
                    sprite_dims: self.sprite_dims.clone(),
                    settings: self.settings,
                };
                view.into_widget(Message::View2D)
            }
            (Mode::View3D, Some(_)) => self.view3d_widget(),
        };
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn bottom_panel(&self) -> Option<Element<'_, Message>> {
        let map = self.map.as_ref()?;

        let single = if self.selection.len() == 1 {
            self.selection.iter().next().copied()
        } else {
            None
        };

        let details_body: Element<'_, Message> = if self.selection.is_empty() {
            column![
                text("No selection").size(15),
                text("Click an element to inspect.").size(12),
                text("Drag an empty area to box-select.").size(12),
                text("Shift+click adds, Esc clears.").size(12),
            ]
            .spacing(2)
            .into()
        } else if let Some(h) = single {
            match h {
                HighlightKind::Sector(_) => self.sector_inspector(map, h),
                HighlightKind::Linedef(id) => self.linedef_inspector(map, id),
                HighlightKind::Thing(id) => self.thing_inspector(map, id),
                _ => selection_details(map, &self.config, h),
            }
        } else {
            multi_select_summary(&self.selection)
        };
        let details = container(details_body)
            .width(Length::Fixed(320.0))
            .padding(10);

        let texture_panels: Element<'_, Message> = match single {
            Some(HighlightKind::Linedef(id)) => {
                let line = map.linedefs.get(id);
                let front = line
                    .and_then(|l| l.right)
                    .and_then(|sid| map.sidedefs.get(sid).map(|s| (sid, s)));
                let back = line
                    .and_then(|l| l.left)
                    .and_then(|sid| map.sidedefs.get(sid).map(|s| (sid, s)));
                row![
                    side_panel("Front Side", front, &self.texture_handles),
                    side_panel("Back Side", back, &self.texture_handles),
                ]
                .spacing(10)
                .into()
            }
            Some(HighlightKind::Sector(id)) => {
                let sec = map.sectors.get(id);
                row![sector_texture_panel(id, sec, &self.texture_handles)]
                    .spacing(10)
                    .into()
            }
            Some(HighlightKind::Thing(id)) => {
                row![thing_preview_panel(map, &self.config, &self.sprite_handles, id)]
                    .spacing(10)
                    .into()
            }
            _ => row![
                side_panel("Front Side", None, &self.texture_handles),
                side_panel("Back Side", None, &self.texture_handles),
            ]
            .spacing(10)
            .into(),
        };

        Some(
            container(
                row![details, texture_panels]
                    .spacing(10)
                    .padding(8),
            )
            .style(panel_style)
            .width(Length::Fill)
            .height(Length::Fixed(160.0))
            .into(),
        )
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let stats = self
            .map_stats
            .as_ref()
            .map(|s| {
                let fmt = match s.format {
                    MapFormat::Doom => "Doom",
                    MapFormat::Hexen => "Hexen",
                };
                format!(
                    "{fmt} | mode: {} | {} verts | {} lines | {} sides | {} sectors | {} things",
                    self.edit_mode.label(),
                    s.vertices,
                    s.linedefs,
                    s.sidedefs,
                    s.sectors,
                    s.things
                )
            })
            .unwrap_or_else(|| self.status.clone());
        let grid = self.camera2d.grid_step();
        let zoom = self.camera2d.zoom;
        let right = if self.map.is_some() {
            let sel = self.selection.len();
            let sel_part = if sel > 0 {
                format!("Sel: {sel}   ")
            } else {
                String::new()
            };
            format!("{sel_part}Grid: {grid:.0}   Zoom: {zoom:.3}")
        } else {
            String::new()
        };
        container(
            row![
                text(stats).size(13),
                Space::new().width(Length::Fill),
                text(right).size(13),
            ]
            .padding(6)
            .align_y(iced::Alignment::Center),
        )
        .style(status_bar_style)
        .width(Length::Fill)
        .into()
    }

    fn picker_modal(&self) -> Element<'_, Message> {
        let backdrop = mouse_area(
            container(Space::new().width(Length::Fill).height(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(modal_backdrop_style),
        )
        .on_press(Message::ClosePicker);

        let body: Element<'_, Message> = match self.active_picker {
            Some(ActivePicker::Texture(_)) => self.texture_picker_panel(),
            Some(ActivePicker::Action(_)) => self.action_picker_panel(),
            Some(ActivePicker::ThingKind(_)) => self.thing_kind_picker_panel(),
            Some(ActivePicker::SectorSpecial(_)) => self.sector_special_picker_panel(),
            Some(ActivePicker::Settings) => self.settings_panel(),
            None => Space::new().into(),
        };

        let panel = container(body)
            .width(Length::Fixed(760.0))
            .height(Length::Fixed(560.0))
            .style(modal_panel_style);

        let centered = container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill);

        stack![backdrop, centered].into()
    }

    fn texture_picker_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Pick a texture").size(18),
            Space::new().width(Length::Fill),
            button("Close").style(style::win32_standard_button).on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let search = text_input("Filter…", &self.picker_filter)
            .on_input(Message::PickerFilterChanged)
            .padding(6)
            .style(style::win32_text_input)
            .width(Length::Fill);

        let q = self.picker_filter.to_ascii_uppercase();
        let filtered: Vec<&String> = self
            .sorted_texture_names
            .iter()
            .filter(|n| q.is_empty() || n.contains(&q))
            .collect();

        let count = text(if self.sorted_texture_names.is_empty() {
            "No textures loaded.".to_string()
        } else {
            format!(
                "{} of {} textures",
                filtered.len(),
                self.sorted_texture_names.len()
            )
        })
        .size(12);

        const COLS: usize = 5;
        const TILE: f32 = 96.0;

        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        let mut current_row: Vec<Element<'_, Message>> = Vec::with_capacity(COLS);
        for name in &filtered {
            let Some(handle) = self.texture_handles.get(*name) else {
                continue;
            };
            let tile = column![
                container(
                    image(handle.clone())
                        .width(Length::Fixed(TILE))
                        .height(Length::Fixed(TILE))
                )
                .width(Length::Fixed(TILE))
                .height(Length::Fixed(TILE))
                .center_x(Length::Fixed(TILE))
                .center_y(Length::Fixed(TILE))
                .style(texture_slot_style),
                text((*name).clone()).size(11),
            ]
            .spacing(2)
            .align_x(iced::Alignment::Center);
            let pickable: Element<'_, Message> = button(tile)
                .padding(2)
                .style(style::win32_toolbar_button)
                .on_press(Message::PickTexture((*name).clone()))
                .into();
            current_row.push(pickable);
            if current_row.len() == COLS {
                let r = std::mem::replace(&mut current_row, Vec::with_capacity(COLS));
                rows.push(row(r).spacing(8).into());
            }
        }
        if !current_row.is_empty() {
            rows.push(row(current_row).spacing(8).into());
        }

        let grid: Element<'_, Message> = if rows.is_empty() {
            container(text("No textures match.").size(13))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        } else {
            scrollable(column(rows).spacing(8).padding(8))
                .height(Length::Fill)
                .into()
        };

        column![title_row, search, count, grid]
            .spacing(8)
            .padding(12)
            .into()
    }

    fn action_picker_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Pick a linedef action").size(18),
            Space::new().width(Length::Fill),
            button("Close").style(style::win32_standard_button).on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let search = text_input("Filter\u{2026}", &self.picker_filter)
            .on_input(Message::PickerFilterChanged)
            .padding(6)
            .style(style::win32_text_input)
            .width(Length::Fill);

        let q = self.picker_filter.to_ascii_lowercase();
        let mut entries: Vec<&doombuilder_core::config::LinedefSpecial> =
            self.config.linedef_specials.iter().collect();
        entries.sort_by_key(|e| e.id);
        let filtered: Vec<&doombuilder_core::config::LinedefSpecial> = entries
            .into_iter()
            .filter(|e| {
                if q.is_empty() {
                    return true;
                }
                e.title.to_ascii_lowercase().contains(&q)
                    || e.category.to_ascii_lowercase().contains(&q)
                    || e.id.to_string().contains(&q)
            })
            .collect();

        let count_text = text(format!(
            "{} of {} actions",
            filtered.len(),
            self.config.linedef_specials.len()
        ))
        .size(12);

        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        let mut last_category: Option<&str> = None;
        for entry in &filtered {
            if last_category.map(|c| c != entry.category.as_str()).unwrap_or(true)
                && !entry.category.is_empty()
            {
                last_category = Some(entry.category.as_str());
                rows.push(
                    text(entry.category.to_string())
                        .size(13)
                        .color(Color::from_rgb(0.7, 0.85, 1.0))
                        .into(),
                );
            }
            let label = if entry.prefix.is_empty() {
                format!("{:>4} - {}", entry.id, entry.title)
            } else {
                format!("{:>4} - {} {}", entry.id, entry.prefix, entry.title)
            };
            let row_btn = button(text(label).size(13))
                .padding(4)
                .style(style::win32_toolbar_button)
                .on_press(Message::PickAction(entry.id))
                .width(Length::Fill);
            rows.push(row_btn.into());
        }

        let list: Element<'_, Message> = if rows.is_empty() {
            container(text("No actions match.").size(13))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        } else {
            scrollable(column(rows).spacing(2).padding(4))
                .height(Length::Fill)
                .into()
        };

        column![title_row, search, count_text, list]
            .spacing(8)
            .padding(12)
            .into()
    }

    fn settings_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Settings").size(18),
            Space::new().width(Length::Fill),
            button("Close").style(style::win32_standard_button).on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let keys = [
            SettingKey::ShowTextures,
            SettingKey::ShowSprites,
            SettingKey::ShowGrid,
            SettingKey::ShowThings,
            SettingKey::AlwaysShowVertices,
        ];

        let rows: Vec<Element<'_, Message>> = keys
            .iter()
            .copied()
            .map(|k| {
                let on = k.get(&self.settings);
                checkbox(on)
                    .label(k.label())
                    .on_toggle(move |v| Message::SetSetting(k, v))
                    .into()
            })
            .collect();

        column![
            title_row,
            text("2D viewport display").size(14),
            column(rows).spacing(6),
        ]
        .spacing(12)
        .padding(16)
        .into()
    }

    fn sector_special_picker_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Pick a sector special").size(18),
            Space::new().width(Length::Fill),
            button("Close").style(style::win32_standard_button).on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let search = text_input("Filter\u{2026}", &self.picker_filter)
            .on_input(Message::PickerFilterChanged)
            .padding(6)
            .style(style::win32_text_input)
            .width(Length::Fill);

        let q = self.picker_filter.to_ascii_lowercase();
        let mut entries: Vec<(u16, &String)> = self
            .config
            .sector_specials
            .iter()
            .filter_map(|(k, v)| k.parse::<u16>().ok().map(|id| (id, v)))
            .collect();
        entries.sort_by_key(|(id, _)| *id);
        let filtered: Vec<(u16, &String)> = entries
            .into_iter()
            .filter(|(id, name)| {
                if q.is_empty() {
                    return true;
                }
                name.to_ascii_lowercase().contains(&q) || id.to_string().contains(&q)
            })
            .collect();

        let count_text = text(format!(
            "{} of {} specials",
            filtered.len(),
            self.config.sector_specials.len()
        ))
        .size(12);

        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        for (id, name) in &filtered {
            let label = format!("{:>4} - {}", id, name);
            let row_btn = button(text(label).size(13))
                .padding(4)
                .style(style::win32_toolbar_button)
                .on_press(Message::PickSectorSpecial(*id))
                .width(Length::Fill);
            rows.push(row_btn.into());
        }

        let list: Element<'_, Message> = if rows.is_empty() {
            container(text("No specials match.").size(13))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        } else {
            scrollable(column(rows).spacing(2).padding(4))
                .height(Length::Fill)
                .into()
        };

        column![title_row, search, count_text, list]
            .spacing(8)
            .padding(12)
            .into()
    }

    fn thing_kind_picker_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Pick a thing type").size(18),
            Space::new().width(Length::Fill),
            button("Close").style(style::win32_standard_button).on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let search = text_input("Filter\u{2026}", &self.picker_filter)
            .on_input(Message::PickerFilterChanged)
            .padding(6)
            .style(style::win32_text_input)
            .width(Length::Fill);

        let q = self.picker_filter.to_ascii_lowercase();
        let mut entries: Vec<&doombuilder_core::config::ThingType> =
            self.config.thing_types.iter().collect();
        entries.sort_by_key(|e| e.id);
        let filtered: Vec<&doombuilder_core::config::ThingType> = entries
            .into_iter()
            .filter(|e| {
                if q.is_empty() {
                    return true;
                }
                e.title.to_ascii_lowercase().contains(&q)
                    || e.category.to_ascii_lowercase().contains(&q)
                    || e.id.to_string().contains(&q)
            })
            .collect();

        let count_text = text(format!(
            "{} of {} things",
            filtered.len(),
            self.config.thing_types.len()
        ))
        .size(12);

        const COLS: usize = 5;
        const TILE: f32 = 96.0;

        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        let mut current_row: Vec<Element<'_, Message>> = Vec::with_capacity(COLS);
        for entry in &filtered {
            let preview: Element<'_, Message> = match find_sprite_handle(
                &self.config,
                &self.sprite_handles,
                entry.id,
            ) {
                Some(handle) => container(
                    image(handle.clone())
                        .width(Length::Fixed(TILE))
                        .height(Length::Fixed(TILE)),
                )
                .width(Length::Fixed(TILE))
                .height(Length::Fixed(TILE))
                .center_x(Length::Fixed(TILE))
                .center_y(Length::Fixed(TILE))
                .style(texture_slot_style)
                .into(),
                None => container(text(format!("kind {}", entry.id)).size(11))
                    .width(Length::Fixed(TILE))
                    .height(Length::Fixed(TILE))
                    .center_x(Length::Fixed(TILE))
                    .center_y(Length::Fixed(TILE))
                    .style(texture_slot_style)
                    .into(),
            };
            let label = format!("{} - {}", entry.id, entry.title);
            let tile = column![preview, text(label).size(10)]
                .spacing(2)
                .align_x(iced::Alignment::Center);
            let pickable: Element<'_, Message> = button(tile)
                .padding(2)
                .style(style::win32_toolbar_button)
                .on_press(Message::PickThingKind(entry.id))
                .into();
            current_row.push(pickable);
            if current_row.len() == COLS {
                let r = std::mem::replace(&mut current_row, Vec::with_capacity(COLS));
                rows.push(row(r).spacing(8).into());
            }
        }
        if !current_row.is_empty() {
            rows.push(row(current_row).spacing(8).into());
        }

        let grid: Element<'_, Message> = if rows.is_empty() {
            container(text("No types match.").size(13))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        } else {
            scrollable(column(rows).spacing(8).padding(8))
                .height(Length::Fill)
                .into()
        };

        column![title_row, search, count_text, grid]
            .spacing(8)
            .padding(12)
            .into()
    }

    fn linedef_inspector(
        &self,
        map: &Map,
        id: doombuilder_core::map::LinedefId,
    ) -> Element<'_, Message> {
        let Some(line) = map.linedefs.get(id) else {
            return text("(missing linedef)").into();
        };
        let Some(buf) = self.linedef_buffers.as_ref() else {
            return text("Loading...").into();
        };
        let length = match (map.vertices.get(line.v1), map.vertices.get(line.v2)) {
            (Some(a), Some(b)) => {
                let dx = (b.x - a.x) as f32;
                let dy = (b.y - a.y) as f32;
                (dx * dx + dy * dy).sqrt()
            }
            _ => 0.0,
        };
        let action_label = match self.config.linedef_special(line.special) {
            Some(s) if !s.prefix.is_empty() => {
                format!("{} - {} {}", line.special, s.prefix, s.title)
            }
            Some(s) => format!("{} - {}", line.special, s.title),
            None => format!("{} - (unknown)", line.special),
        };
        let front_sec = line
            .right
            .and_then(|sid| map.sidedefs.get(sid))
            .map(|s| s.sector);
        let back_sec = line
            .left
            .and_then(|sid| map.sidedefs.get(sid))
            .map(|s| s.sector);
        let (fh, ch) = sector_heights(map, front_sec);
        let (bfh, bch) = sector_heights(map, back_sec);

        let flags = flag_toggle_row(&self.config.linedef_flags, line.flags, move |bit| {
            Message::ToggleLinedefFlag { id, bit }
        });

        let row_input = |label: &'static str, val: String, field: LinedefIntField| {
            row![
                container(text(label).size(13)).width(Length::Fixed(72.0)),
                text_input("0", &val)
                    .on_input(move |t| Message::LinedefFieldChanged { field, text: t })
                    .on_submit(Message::LinedefFieldSubmit(field))
                    .padding(4)
            .style(style::win32_text_input)
                    .width(Length::Fixed(80.0)),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center)
        };

        let mut col = column![
            text("Linedef").size(15),
            button(text(format!("Action:  {action_label}")).size(13))
                .padding(0)
                .style(style::win32_toolbar_button)
                .on_press(Message::OpenActionPicker(id)),
            text(format!("Length:  {length:.0}")).size(13),
            row_input("Tag:", buf.tag.clone(), LinedefIntField::Tag),
            text("Flags:").size(13),
            flags,
        ]
        .spacing(4);

        if map.format == MapFormat::Hexen {
            col = col.push(text("Args:").size(13));
            col = col.push(
                row![
                    text_input("0", &buf.args[0])
                        .on_input(|t| Message::LinedefFieldChanged {
                            field: LinedefIntField::Arg0,
                            text: t,
                        })
                        .on_submit(Message::LinedefFieldSubmit(LinedefIntField::Arg0))
                        .padding(4)
            .style(style::win32_text_input)
                        .width(Length::Fixed(56.0)),
                    text_input("0", &buf.args[1])
                        .on_input(|t| Message::LinedefFieldChanged {
                            field: LinedefIntField::Arg1,
                            text: t,
                        })
                        .on_submit(Message::LinedefFieldSubmit(LinedefIntField::Arg1))
                        .padding(4)
            .style(style::win32_text_input)
                        .width(Length::Fixed(56.0)),
                    text_input("0", &buf.args[2])
                        .on_input(|t| Message::LinedefFieldChanged {
                            field: LinedefIntField::Arg2,
                            text: t,
                        })
                        .on_submit(Message::LinedefFieldSubmit(LinedefIntField::Arg2))
                        .padding(4)
            .style(style::win32_text_input)
                        .width(Length::Fixed(56.0)),
                    text_input("0", &buf.args[3])
                        .on_input(|t| Message::LinedefFieldChanged {
                            field: LinedefIntField::Arg3,
                            text: t,
                        })
                        .on_submit(Message::LinedefFieldSubmit(LinedefIntField::Arg3))
                        .padding(4)
            .style(style::win32_text_input)
                        .width(Length::Fixed(56.0)),
                    text_input("0", &buf.args[4])
                        .on_input(|t| Message::LinedefFieldChanged {
                            field: LinedefIntField::Arg4,
                            text: t,
                        })
                        .on_submit(Message::LinedefFieldSubmit(LinedefIntField::Arg4))
                        .padding(4)
            .style(style::win32_text_input)
                        .width(Length::Fixed(56.0)),
                ]
                .spacing(4),
            );
        }

        col = col.push(
            text(format!(
                "Front Sector: {}    Back Sector: {}",
                sector_label(front_sec),
                sector_label(back_sec)
            ))
            .size(13),
        );
        col = col.push(
            text(format!(
                "Front Height: {fh}/{ch}    Back Height: {bfh}/{bch}"
            ))
            .size(13),
        );

        col.into()
    }

    fn thing_inspector(&self, map: &Map, id: ThingId) -> Element<'_, Message> {
        let Some(t) = map.things.get(id) else {
            return text("(missing thing)").into();
        };
        let Some(buf) = self.thing_buffers.as_ref() else {
            return text("Loading...").into();
        };
        let name = match self.config.thing_type(t.kind) {
            Some(tt) => format!("{} - {}", t.kind, tt.title),
            None => format!("{} - (unknown)", t.kind),
        };
        let flags = flag_toggle_row(&self.config.thing_flags, t.flags, move |bit| {
            Message::ToggleThingFlag { id, bit }
        });

        let angle_row = row![
            container(text("Angle:").size(13)).width(Length::Fixed(72.0)),
            text_input("0", &buf.angle)
                .on_input(|t| Message::ThingFieldChanged {
                    field: ThingIntField::Angle,
                    text: t,
                })
                .on_submit(Message::ThingFieldSubmit(ThingIntField::Angle))
                .padding(4)
            .style(style::win32_text_input)
                .width(Length::Fixed(80.0)),
            text("\u{00B0}").size(13),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        column![
            text("Thing").size(15),
            button(text(format!("Type:    {name}")).size(13))
                .padding(0)
                .style(style::win32_toolbar_button)
                .on_press(Message::OpenThingKindPicker(id)),
            text(format!("X:       {}", t.x)).size(13),
            text(format!("Y:       {}", t.y)).size(13),
            angle_row,
            text("Flags:").size(13),
            flags,
        ]
        .spacing(4)
        .into()
    }

    fn sector_inspector(&self, map: &Map, highlight: HighlightKind) -> Element<'_, Message> {
        let id = match highlight {
            HighlightKind::Sector(id) => id,
            _ => return text("(internal: not a sector)").into(),
        };
        let Some(sec) = map.sectors.get(id) else {
            return text("(missing sector)").into();
        };
        let Some(buf) = self.sector_buffers.as_ref() else {
            return text("Loading...").into();
        };

        let row_input = |label: &'static str, val: String, field: SectorIntField| {
            row![
                container(text(label).size(13)).width(Length::Fixed(72.0)),
                text_input("0", &val)
                    .on_input(move |t| Message::SectorFieldChanged { field, text: t })
                    .on_submit(Message::SectorFieldSubmit(field))
                    .padding(4)
            .style(style::win32_text_input)
                    .width(Length::Fixed(120.0)),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center)
        };

        let special_label = match self.config.sector_special(sec.special) {
            Some(name) => format!("{} - {}", sec.special, name),
            None => format!("{} - (unknown)", sec.special),
        };

        column![
            text("Sector").size(15),
            row_input("Floor:", buf.floor.clone(), SectorIntField::FloorHeight),
            row_input("Ceiling:", buf.ceiling.clone(), SectorIntField::CeilingHeight),
            row_input("Light:", buf.light.clone(), SectorIntField::Light),
            row_input("Tag:", buf.tag.clone(), SectorIntField::Tag),
            button(text(format!("Special: {special_label}")).size(13))
                .padding(0)
                .style(style::win32_toolbar_button)
                .on_press(Message::OpenSectorSpecialPicker(id)),
            text(format!("Sidedefs: {}", sec.sidedefs.len())).size(13),
        ]
        .spacing(4)
        .into()
    }

    fn view3d_widget(&self) -> Element<'_, Message> {
        let textures = match &self.textures {
            Some(t) => t.clone(),
            None => Arc::new(TextureSet::empty(Vec::new())),
        };
        let view = View3D {
            geometry: self.geometry3d.clone(),
            textures,
            camera: self.camera3d,
        };
        view.into_widget(Message::View3D)
    }
}

fn multi_select_summary<'a>(selection: &HashSet<HighlightKind>) -> Element<'a, Message> {
    let mut verts = 0usize;
    let mut lines = 0usize;
    let mut sectors = 0usize;
    let mut things = 0usize;
    for h in selection {
        match h {
            HighlightKind::Vertex(_) => verts += 1,
            HighlightKind::Linedef(_) => lines += 1,
            HighlightKind::Sector(_) => sectors += 1,
            HighlightKind::Thing(_) => things += 1,
        }
    }
    column![
        text(format!("{} elements selected", selection.len())).size(15),
        text(format!("Vertices: {verts}")).size(13),
        text(format!("Linedefs: {lines}")).size(13),
        text(format!("Sectors:  {sectors}")).size(13),
        text(format!("Things:   {things}")).size(13),
    ]
    .spacing(2)
    .into()
}

fn selection_details<'a>(
    map: &Map,
    config: &GameConfig,
    highlight: HighlightKind,
) -> Element<'a, Message> {
    match highlight {
        HighlightKind::Vertex(id) => {
            let mut col = column![text("Vertex").size(15)].spacing(2);
            if let Some(v) = map.vertices.get(id) {
                col = col.push(text(format!("x: {}", v.x)));
                col = col.push(text(format!("y: {}", v.y)));
            }
            col.into()
        }
        HighlightKind::Linedef(id) => {
            let mut col = column![text("Linedef").size(15)].spacing(2);
            if let Some(l) = map.linedefs.get(id) {
                let length = match (map.vertices.get(l.v1), map.vertices.get(l.v2)) {
                    (Some(a), Some(b)) => {
                        let dx = (b.x - a.x) as f32;
                        let dy = (b.y - a.y) as f32;
                        (dx * dx + dy * dy).sqrt()
                    }
                    _ => 0.0,
                };
                let action_label = match config.linedef_special(l.special) {
                    Some(s) if !s.prefix.is_empty() => {
                        format!("{} - {} {}", l.special, s.prefix, s.title)
                    }
                    Some(s) => format!("{} - {}", l.special, s.title),
                    None => format!("{} - (unknown)", l.special),
                };
                col = col.push(
                    button(text(format!("Action:  {action_label}")))
                        .padding(0)
                        .style(style::win32_toolbar_button)
                        .on_press(Message::OpenActionPicker(id)),
                );
                col = col.push(text(format!("Length:  {length:.0}")));
                col = col.push(text(format!("Tag:     {}", l.tag)));
                col = col.push(text(format!(
                    "Flags:   {}",
                    config.format_linedef_flags(l.flags)
                )));
                let front_sec = l
                    .right
                    .and_then(|sid| map.sidedefs.get(sid))
                    .map(|s| s.sector);
                let back_sec = l
                    .left
                    .and_then(|sid| map.sidedefs.get(sid))
                    .map(|s| s.sector);
                col = col.push(text(format!(
                    "Front Sector: {}    Back Sector: {}",
                    sector_label(front_sec),
                    sector_label(back_sec)
                )));
                let (fh, ch) = sector_heights(map, front_sec);
                let (bfh, bch) = sector_heights(map, back_sec);
                col = col.push(text(format!(
                    "Front Height: {fh}/{ch}    Back Height: {bfh}/{bch}"
                )));
            }
            col.into()
        }
        HighlightKind::Thing(id) => {
            let mut col = column![text("Thing").size(15)].spacing(2);
            if let Some(t) = map.things.get(id) {
                let name = match config.thing_type(t.kind) {
                    Some(tt) => format!("{} - {}", t.kind, tt.title),
                    None => format!("{} - (unknown)", t.kind),
                };
                col = col.push(
                    button(text(format!("Type:    {name}")))
                        .padding(0)
                        .style(style::win32_toolbar_button)
                        .on_press(Message::OpenThingKindPicker(id)),
                );
                col = col.push(text(format!("X:       {}", t.x)));
                col = col.push(text(format!("Y:       {}", t.y)));
                col = col.push(text(format!("Angle:   {}\u{00B0}", t.angle)));
                col = col.push(text(format!(
                    "Flags:   {}",
                    config.format_thing_flags(t.flags)
                )));
            }
            col.into()
        }
        HighlightKind::Sector(id) => {
            let mut col = column![text("Sector").size(15)].spacing(2);
            if let Some(s) = map.sectors.get(id) {
                let special_label = match config.sector_special(s.special) {
                    Some(name) => format!("{} - {}", s.special, name),
                    None => format!("{} - (unknown)", s.special),
                };
                col = col.push(text(format!("Floor:   {}", s.floor_height)));
                col = col.push(text(format!("Ceiling: {}", s.ceiling_height)));
                col = col.push(text(format!("Light:   {}", s.light)));
                col = col.push(text(format!("Special: {special_label}")));
                col = col.push(text(format!("Tag:     {}", s.tag)));
                col = col.push(text(format!("Floor tex:   {}", s.floor_texture.as_str())));
                col = col.push(text(format!("Ceil tex:    {}", s.ceiling_texture.as_str())));
                col = col.push(text(format!("Sidedefs:    {}", s.sidedefs.len())));
            }
            col.into()
        }
    }
}

fn sector_label(s: Option<SectorId>) -> String {
    match s {
        Some(id) => format!("{id:?}"),
        None => "-".into(),
    }
}

fn sector_heights(map: &Map, s: Option<SectorId>) -> (String, String) {
    match s.and_then(|id| map.sectors.get(id)) {
        Some(sec) => (sec.floor_height.to_string(), sec.ceiling_height.to_string()),
        None => ("-".into(), "-".into()),
    }
}

fn side_panel<'a>(
    title: &'a str,
    side_with_id: Option<(doombuilder_core::map::SidedefId, &MapSidedef)>,
    handles: &HashMap<String, ImageHandle>,
) -> Element<'a, Message> {
    let slots: Element<'_, Message> = match side_with_id {
        Some((id, side)) => row![
            texture_slot(
                "Upper",
                side.upper_texture,
                handles,
                Some(PickerTarget::Sidedef {
                    sidedef: id,
                    slot: SidedefSlot::Upper,
                }),
            ),
            texture_slot(
                "Middle",
                side.middle_texture,
                handles,
                Some(PickerTarget::Sidedef {
                    sidedef: id,
                    slot: SidedefSlot::Middle,
                }),
            ),
            texture_slot(
                "Lower",
                side.lower_texture,
                handles,
                Some(PickerTarget::Sidedef {
                    sidedef: id,
                    slot: SidedefSlot::Lower,
                }),
            ),
        ]
        .spacing(8)
        .into(),
        None => text("(none)").size(13).into(),
    };
    container(column![text(title).size(14), slots].spacing(6))
        .padding(8)
        .style(side_panel_style)
        .into()
}

fn thing_preview_panel<'a>(
    map: &Map,
    config: &GameConfig,
    sprite_handles: &HashMap<String, ImageHandle>,
    id: ThingId,
) -> Element<'a, Message> {
    let body: Element<'_, Message> = match map.things.get(id) {
        Some(t) => {
            let title = config
                .thing_type(t.kind)
                .map(|tt| tt.title.clone())
                .unwrap_or_else(|| format!("Thing {}", t.kind));
            let preview: Element<'_, Message> = match find_sprite_handle(config, sprite_handles, t.kind)
            {
                Some(handle) => container(
                    image(handle.clone())
                        .width(Length::Fixed(96.0))
                        .height(Length::Fixed(96.0)),
                )
                .width(Length::Fixed(96.0))
                .height(Length::Fixed(96.0))
                .center_x(Length::Fixed(96.0))
                .center_y(Length::Fixed(96.0))
                .style(texture_slot_style)
                .into(),
                None => container(text(format!("kind {}", t.kind)).size(11))
                    .width(Length::Fixed(96.0))
                    .height(Length::Fixed(96.0))
                    .center_x(Length::Fixed(96.0))
                    .center_y(Length::Fixed(96.0))
                    .style(texture_slot_style)
                    .into(),
            };
            column![preview, text(title).size(11)]
                .spacing(2)
                .align_x(iced::Alignment::Center)
                .into()
        }
        None => text("(missing)").size(13).into(),
    };
    container(column![text("Sprite").size(14), body].spacing(6))
        .padding(8)
        .style(side_panel_style)
        .into()
}

fn find_sprite_handle<'a>(
    config: &GameConfig,
    handles: &'a HashMap<String, ImageHandle>,
    kind: u16,
) -> Option<&'a ImageHandle> {
    let raw = config.thing_type(kind)?.sprite.to_ascii_uppercase();
    if raw.is_empty() {
        return None;
    }
    let take_n = |n| raw.chars().take(n).collect::<String>();
    let base4 = take_n(4);
    let candidates = [
        take_n(6),
        format!("{base4}A0"),
        format!("{base4}A1"),
        format!("{base4}A2"),
    ];
    for c in candidates {
        if let Some(h) = handles.get(&c) {
            return Some(h);
        }
    }
    None
}

fn sector_texture_panel<'a>(
    id: SectorId,
    sector: Option<&doombuilder_core::map::MapSector>,
    handles: &HashMap<String, ImageHandle>,
) -> Element<'a, Message> {
    let slots: Element<'_, Message> = match sector {
        Some(sec) => row![
            texture_slot(
                "Floor",
                sec.floor_texture,
                handles,
                Some(PickerTarget::Sector {
                    sector: id,
                    slot: SectorSlot::Floor,
                }),
            ),
            texture_slot(
                "Ceiling",
                sec.ceiling_texture,
                handles,
                Some(PickerTarget::Sector {
                    sector: id,
                    slot: SectorSlot::Ceiling,
                }),
            ),
        ]
        .spacing(8)
        .into(),
        None => text("(none)").size(13).into(),
    };
    container(column![text("Sector flats").size(14), slots].spacing(6))
        .padding(8)
        .style(side_panel_style)
        .into()
}

fn texture_slot<'a>(
    label: &'a str,
    name: TextureName,
    handles: &HashMap<String, ImageHandle>,
    target: Option<PickerTarget>,
) -> Element<'a, Message> {
    let displayed = name.as_str().to_ascii_uppercase();
    let is_missing = displayed.is_empty() || displayed == "-";

    let body: Element<'_, Message> = if is_missing {
        container(
            text("Missing")
                .size(12)
                .color(Color::from_rgb(0.85, 0.45, 0.45)),
        )
        .width(Length::Fixed(72.0))
        .height(Length::Fixed(72.0))
        .center_x(Length::Fixed(72.0))
        .center_y(Length::Fixed(72.0))
        .style(texture_slot_style)
        .into()
    } else if let Some(handle) = handles.get(&displayed) {
        container(image(handle.clone()).width(Length::Fixed(72.0)).height(Length::Fixed(72.0)))
            .width(Length::Fixed(72.0))
            .height(Length::Fixed(72.0))
            .center_x(Length::Fixed(72.0))
            .center_y(Length::Fixed(72.0))
            .style(texture_slot_style)
            .into()
    } else {
        container(text(displayed.clone()).size(11).color(Color::from_rgb(0.9, 0.9, 0.9)))
            .width(Length::Fixed(72.0))
            .height(Length::Fixed(72.0))
            .center_x(Length::Fixed(72.0))
            .center_y(Length::Fixed(72.0))
            .style(texture_slot_style)
            .into()
    };

    let slot: Element<'_, Message> = if let Some(t) = target {
        button(body)
            .padding(0)
            .style(style::win32_toolbar_button)
            .on_press(Message::OpenTexturePicker(t))
            .into()
    } else {
        body
    };

    column![slot, text(label).size(11)]
        .spacing(2)
        .align_x(iced::Alignment::Center)
        .into()
}

fn vertical_separator() -> Element<'static, Message> {
    container(Space::new().width(Length::Fixed(1.0)))
        .height(Length::Fixed(20.0))
        .style(separator_style)
        .into()
}

// ---- Menu items -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct MenuItem(&'static str);

impl std::fmt::Display for MenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

const FILE_MENU_ITEMS: &[MenuItem] =
    &[MenuItem("Open WAD…"), MenuItem("Save Map As…"), MenuItem("Quit")];
const EDIT_MENU_ITEMS: &[MenuItem] = &[
    MenuItem("Undo"),
    MenuItem("Redo"),
    MenuItem("Select All"),
    MenuItem("Clear Selection"),
    MenuItem("Delete Selection"),
    MenuItem("Insert Thing"),
    MenuItem("Make Sector"),
    MenuItem("Toggle Draw Mode"),
];
const VIEW_MENU_ITEMS: &[MenuItem] =
    &[MenuItem("2D Mode"), MenuItem("3D Mode"), MenuItem("Settings\u{2026}")];
const TOOLS_MENU_ITEMS: &[MenuItem] = &[MenuItem("Map Statistics (n/a)")];
const HELP_MENU_ITEMS: &[MenuItem] = &[MenuItem("About (n/a)")];

fn dispatch_file(item: MenuItem) -> Message {
    match item.0 {
        "Open WAD…" => Message::OpenWadRequested,
        "Save Map As…" => Message::SaveMapRequested,
        "Quit" => Message::Quit,
        _ => Message::Noop,
    }
}

fn dispatch_edit(item: MenuItem) -> Message {
    match item.0 {
        "Undo" => Message::Undo,
        "Redo" => Message::Redo,
        "Select All" => Message::SelectAll,
        "Clear Selection" => Message::KeyboardEsc,
        "Delete Selection" => Message::DeleteSelection,
        "Insert Thing" => Message::InsertThing,
        "Make Sector" => Message::MakeSector,
        "Toggle Draw Mode" => Message::ToggleDrawing,
        _ => Message::Noop,
    }
}

fn dispatch_view(item: MenuItem) -> Message {
    match item.0 {
        "2D Mode" => Message::Mode(Mode::View2D),
        "3D Mode" => Message::Mode(Mode::View3D),
        "Settings\u{2026}" => Message::OpenSettings,
        _ => Message::Noop,
    }
}

fn dispatch_tools(_item: MenuItem) -> Message {
    Message::Noop
}

fn dispatch_help(_item: MenuItem) -> Message {
    Message::Noop
}

fn menu_picker(
    label: &'static str,
    items: &'static [MenuItem],
    on_pick: fn(MenuItem) -> Message,
) -> Element<'static, Message> {
    pick_list(items, None::<MenuItem>, on_pick)
        .placeholder(label)
        .into()
}

fn flag_toggle_row<'a, F>(
    table: &HashMap<String, String>,
    flags: u16,
    on_toggle: F,
) -> Element<'a, Message>
where
    F: Fn(u16) -> Message + 'a,
{
    let mut entries: Vec<(u16, String)> = table
        .iter()
        .filter_map(|(k, v)| k.parse::<u16>().ok().map(|b| (b, v.clone())))
        .collect();
    entries.sort_by_key(|(b, _)| *b);
    let cells: Vec<Element<'_, Message>> = entries
        .into_iter()
        .map(|(bit, label)| {
            let on = (flags & bit) != 0;
            button(text(label).size(11))
                .padding(4)
                .style(style::win32_toggle_button(on))
                .on_press(on_toggle(bit))
                .into()
        })
        .collect();
    column![row(cells).spacing(4).wrap()]
        .spacing(2)
        .into()
}

fn linedef_buffer_field(b: &LinedefBuffers, field: LinedefIntField) -> &str {
    match field {
        LinedefIntField::Tag => &b.tag,
        LinedefIntField::Arg0 => &b.args[0],
        LinedefIntField::Arg1 => &b.args[1],
        LinedefIntField::Arg2 => &b.args[2],
        LinedefIntField::Arg3 => &b.args[3],
        LinedefIntField::Arg4 => &b.args[4],
        LinedefIntField::Flags => "",
    }
}

fn linedef_buffer_field_mut<'a>(
    b: &'a mut LinedefBuffers,
    field: LinedefIntField,
) -> Option<&'a mut String> {
    match field {
        LinedefIntField::Tag => Some(&mut b.tag),
        LinedefIntField::Arg0 => Some(&mut b.args[0]),
        LinedefIntField::Arg1 => Some(&mut b.args[1]),
        LinedefIntField::Arg2 => Some(&mut b.args[2]),
        LinedefIntField::Arg3 => Some(&mut b.args[3]),
        LinedefIntField::Arg4 => Some(&mut b.args[4]),
        LinedefIntField::Flags => None,
    }
}

fn thing_buffer_field(b: &ThingBuffers, field: ThingIntField) -> &str {
    match field {
        ThingIntField::Angle => &b.angle,
        ThingIntField::Flags => "",
    }
}

fn thing_buffer_field_mut<'a>(
    b: &'a mut ThingBuffers,
    field: ThingIntField,
) -> Option<&'a mut String> {
    match field {
        ThingIntField::Angle => Some(&mut b.angle),
        ThingIntField::Flags => None,
    }
}

fn sector_buffer_field(b: &SectorBuffers, field: SectorIntField) -> &str {
    match field {
        SectorIntField::FloorHeight => &b.floor,
        SectorIntField::CeilingHeight => &b.ceiling,
        SectorIntField::Light => &b.light,
        SectorIntField::Tag => &b.tag,
        SectorIntField::Special => "",
    }
}

fn sector_buffer_field_mut<'a>(
    b: &'a mut SectorBuffers,
    field: SectorIntField,
) -> Option<&'a mut String> {
    match field {
        SectorIntField::FloorHeight => Some(&mut b.floor),
        SectorIntField::CeilingHeight => Some(&mut b.ceiling),
        SectorIntField::Light => Some(&mut b.light),
        SectorIntField::Tag => Some(&mut b.tag),
        SectorIntField::Special => None,
    }
}

fn edit_mode_matches(mode: EditMode, h: HighlightKind) -> bool {
    matches!(
        (mode, h),
        (EditMode::Vertices, HighlightKind::Vertex(_))
            | (EditMode::Linedefs, HighlightKind::Linedef(_))
            | (EditMode::Sectors, HighlightKind::Sector(_))
            | (EditMode::Things, HighlightKind::Thing(_))
    )
}

fn menu_bar_style(theme: &Theme) -> container::Style {
    style::win32_menu(theme)
}

fn panel_style(theme: &Theme) -> container::Style {
    style::win32_panel(theme)
}

fn status_bar_style(theme: &Theme) -> container::Style {
    style::win32_status_bar(theme)
}

fn side_panel_style(theme: &Theme) -> container::Style {
    style::win32_side_panel(theme)
}

fn modal_backdrop_style(theme: &Theme) -> container::Style {
    style::win32_modal_backdrop(theme)
}

fn modal_panel_style(theme: &Theme) -> container::Style {
    style::win32_modal_panel(theme)
}

fn texture_slot_style(theme: &Theme) -> container::Style {
    style::win32_well(theme)
}

fn separator_style(theme: &Theme) -> container::Style {
    style::win32_separator(theme)
}

async fn pick_save_path(suggested_stem: String) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_file_name(format!("{suggested_stem}.wad"))
        .add_filter("PWAD", &["wad"])
        .save_file()
        .await
        .map(|h| h.path().to_path_buf())
}

async fn save_map_to_path(map: Arc<Map>, path: PathBuf) -> Result<PathBuf, String> {
    let bytes = save_map_as_pwad(&map);
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

async fn pick_file() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter("Doom assets", &["wad", "pk3", "zip"])
        .add_filter("All files", &["*"])
        .pick_file()
        .await
        .map(|h| h.path().to_path_buf())
}

async fn load_asset(path: PathBuf) -> Result<AssetSummary, String> {
    let (wad, textures, summary, maps) = open_and_summarise(&path).map_err(|e| e.to_string())?;
    let (texture_handles, sprite_handles, sprite_dims) = match textures.as_ref() {
        Some(ts) => (
            build_texture_handles(ts),
            build_sprite_handles(ts),
            build_sprite_dims(ts),
        ),
        None => (HashMap::new(), HashMap::new(), HashMap::new()),
    };
    Ok(AssetSummary {
        path,
        wad,
        textures,
        texture_handles: Arc::new(texture_handles),
        sprite_handles: Arc::new(sprite_handles),
        sprite_dims: Arc::new(sprite_dims),
        summary,
        maps,
    })
}

fn build_sprite_dims(set: &TextureSet) -> HashMap<String, (u32, u32)> {
    set.sprites
        .iter()
        .map(|(name, img)| (name.clone(), (img.width as u32, img.height as u32)))
        .collect()
}

fn build_texture_handles(set: &TextureSet) -> HashMap<String, ImageHandle> {
    let mut out = HashMap::with_capacity(set.textures.len() + set.flats.len());
    for (name, img) in set.textures.iter().chain(set.flats.iter()) {
        let handle = ImageHandle::from_rgba(img.width as u32, img.height as u32, img.rgba.clone());
        out.insert(name.clone(), handle);
    }
    out
}

fn build_sprite_handles(set: &TextureSet) -> HashMap<String, ImageHandle> {
    let mut out = HashMap::with_capacity(set.sprites.len());
    for (name, img) in set.sprites.iter() {
        let handle = ImageHandle::from_rgba(img.width as u32, img.height as u32, img.rgba.clone());
        out.insert(name.clone(), handle);
    }
    out
}

async fn load_map_payload(wad: Wad, name: String) -> Result<MapPayload, String> {
    let map = load_auto(&wad, &name).map_err(|e| e.to_string())?;

    let loops_by_sector = extract_sector_loops(&map);
    let mut meshes_with_id: Vec<(SectorId, FloorMesh)> = Vec::with_capacity(loops_by_sector.len());
    for (sid, loops) in &loops_by_sector {
        if let Ok(mesh) = triangulate_sector(&map, *sid, loops) {
            if !mesh.indices.is_empty() {
                meshes_with_id.push((*sid, mesh));
            }
        }
    }
    let walls = build_walls(&map);
    let spatial = SpatialIndex::build(&map, meshes_with_id.clone());

    let stats = MapStats {
        name: map.name.clone(),
        format: map.format,
        vertices: map.vertices.len(),
        linedefs: map.linedefs.len(),
        sidedefs: map.sidedefs.len(),
        sectors: map.sectors.len(),
        things: map.things.len(),
    };

    Ok(MapPayload {
        map: Arc::new(map),
        sector_meshes: Arc::new(meshes_with_id),
        walls: Arc::new(walls),
        spatial: Arc::new(spatial),
        stats,
    })
}

fn open_and_summarise(
    path: &Path,
) -> Result<
    (Option<Wad>, Option<Arc<TextureSet>>, String, Vec<String>),
    doombuilder_core::Error,
> {
    match open_asset(path)? {
        Asset::Wad(wad) => {
            let textures = Arc::new(TextureSet::load_from_wad(&wad));
            let (summary, maps) = summarise_wad(&wad);
            Ok((Some(wad), Some(textures), summary, maps))
        }
        Asset::Pk3(pk3) => {
            let (summary, maps) = summarise_pk3(&pk3);
            Ok((None, None, summary, maps))
        }
    }
}

fn summarise_wad(wad: &Wad) -> (String, Vec<String>) {
    let kind = match wad.kind() {
        WadKind::Iwad => "IWAD",
        WadKind::Pwad => "PWAD",
    };
    let summary = format!(
        "{kind}: {} lumps, {} bytes",
        wad.directory().len(),
        wad.bytes().len()
    );
    (summary, wad.map_markers())
}

fn summarise_pk3(pk3: &Pk3) -> (String, Vec<String>) {
    let names = pk3.entry_names();
    let total = names.len();
    let maps: Vec<String> = names
        .into_iter()
        .filter(|n| {
            let lower = n.to_ascii_lowercase();
            lower.starts_with("maps/") && (lower.ends_with(".wad") || lower.ends_with(".txt"))
        })
        .collect();
    (format!("PK3: {total} entries"), maps)
}
