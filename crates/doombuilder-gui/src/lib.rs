// ABOUTME: Iced application root for doombuilder-rust.
// ABOUTME: UDB-style layout: dynamic title, toolbar with map picker, full
// ABOUTME: viewport, bottom inspector with texture slots, status bar.

mod camera;
mod icons;
mod palette;
mod style;
mod view2d;
mod view3d;

use palette::ThemeKind;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use doombuilder_core::archive::{open as open_asset, Asset, Pk3};
use doombuilder_core::config::GameConfig;
use doombuilder_core::edit::{
    collect_and_delete, compute_insert_vertex_on_line, compute_make_sector, compute_split_lines,
    compute_vertex_merge, Command,
    LineEndpoint, LinedefChain, LinedefIntField, SectorIntField, SectorSlot, SidedefSlot,
    ThingIntField, ThingMove, UndoStack, VertexMove,
};
use doombuilder_core::map::LinedefId;
use doombuilder_core::map::MapThing;
use doombuilder_core::map::{
    save_map_as_pwad, save_map_as_pwad_with, Map, MapLinedef, MapSidedef, MapVertex, NodeBuilder,
    SectorId, TextureName, ThingId,
    VertexId,
};
use doombuilder_core::textures::TextureSet;
use doombuilder_core::wad::WadKind;
use doombuilder_core::{load_auto, MapFormat, Wad};
use doombuilder_render::{
    build_walls, extract_sector_loops, triangulate_sector, FloorMesh,
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
        // Sized so the full toolbar (icons + menu picker + map picker)
        // fits without horizontal scrolling on standard DPI displays.
        .window_size(iced::Size::new(1160.0, 812.0))
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
    /// Last single-click `(time, world position)`, used purely to detect
    /// double-clicks in the canvas. We do this in App rather than in the
    /// canvas state because dispatch happens in one place and the canvas
    /// has no view into edit mode.
    last_click: Option<(std::time::Instant, Vec2)>,
    /// Edit buffer for the Map Options modal's name field. Live string the
    /// user types; only committed to `map.name` on submit.
    map_name_buffer: String,
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
    /// Buffer state for the Go-To-Coords modal (text in the X/Y inputs).
    go_to_coords_x: String,
    go_to_coords_y: String,
    /// Buffer for the Tag Range modal's starting tag.
    tag_range_input: String,
    /// Hide hover overlays when false (toggled with H).
    show_highlights: bool,
    /// When true, left-mouse drag in the 2D viewport pans instead of selects.
    space_held: bool,
    /// User-placed Visual Mode camera position; takes precedence over the
    /// map AABB centre when entering 3D mode.
    visual_camera_start: Option<Vec2>,
    /// Most-recently copied / cut selection. Survives across maps.
    clipboard: Option<doombuilder_core::edit::ClipboardData>,
    /// Named selection sets (0..=9). Each entry stores a saved selection
    /// snapshot the user can restore with a number-key hotkey.
    selection_groups: [Option<HashSet<HighlightKind>>; 10],
}

#[derive(Debug, Default)]
pub struct DrawingState {
    pub chain: LinedefChain,
    /// Current chain head (live VertexId in the map + how we'd serialise it).
    pub last: Option<(VertexId, LineEndpoint)>,
    /// Active drawing tool. `Free` is the classic click-to-place-vertex flow;
    /// other variants are two- or three-click shape builders that emit a
    /// whole geometry on the final click.
    pub tool: DrawTool,
}

#[derive(Debug, Clone)]
pub enum DrawTool {
    Free,
    Rectangle { origin: Option<Vec2>, bevel: u32 },
    Ellipse { origin: Option<Vec2>, subdivisions: u32 },
    Curve { points: Vec<Vec2>, subdivisions: u32 },
    Grid { origin: Option<Vec2>, cols: u32, rows: u32 },
}

impl Default for DrawTool {
    fn default() -> Self {
        DrawTool::Free
    }
}

impl DrawTool {
    fn label(&self) -> &'static str {
        match self {
            DrawTool::Free => "Free draw",
            DrawTool::Rectangle { .. } => "Rectangle draw",
            DrawTool::Ellipse { .. } => "Ellipse draw",
            DrawTool::Curve { .. } => "Curve draw",
            DrawTool::Grid { .. } => "Grid draw",
        }
    }
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
    GoToCoords,
    MapStats,
    MapAnalysis,
    UsedTags,
    TagRange,
    ThingTypes,
    /// List the maps inside the currently-loaded WAD; clicking one loads it.
    MapInWad,
    /// Editable map metadata: name, format. (F2)
    MapOptions,
    /// Help → About modal.
    About,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    #[serde(default = "default_true")]
    pub snap_to_grid: bool,
    /// Fixed grid spacing in map units, or `None` for the zoom-derived
    /// power-of-two auto step. Cycled by the `G` / `Shift+G` hotkeys.
    #[serde(default)]
    pub grid_size: Option<u32>,
    /// Most-recently opened paths, newest first, capped at MAX_RECENT.
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    /// Doom engine binary used by "Test Map" (chocolate-doom, gzdoom, etc.).
    #[serde(default)]
    pub engine_path: Option<PathBuf>,
    /// IWAD passed to the engine alongside the saved test PWAD.
    #[serde(default)]
    pub iwad_path: Option<PathBuf>,
    /// Active visual theme. Picked from a built-in palette set.
    #[serde(default)]
    pub theme: ThemeKind,
    /// 2D viewport fill mode (Floor textures / Ceiling textures / Brightness
    /// levels / Wireframe).
    #[serde(default)]
    pub view_mode: View2DMode,
    /// Render the 3D view ignoring sector light (light = 255 everywhere).
    #[serde(default)]
    pub full_brightness: bool,
    /// Show the 3D preview in a dedicated right-side panel in 2D mode.
    #[serde(default = "default_true")]
    pub show_3d_overlay: bool,
    /// Which BSP/blockmap/reject builder to use when saving maps.
    #[serde(default)]
    pub node_builder: NodeBuilderKind,
    /// Path to a `zdbsp` executable, used when `node_builder == Zdbsp`.
    #[serde(default)]
    pub zdbsp_path: Option<PathBuf>,
}

/// Serializable choice of node builder. Mirrors `doombuilder_core::map::
/// NodeBuilder` but stays a flat enum so it round-trips through the JSON
/// settings file without needing to embed paths in every variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum NodeBuilderKind {
    #[default]
    Builtin,
    Zdbsp,
}

impl NodeBuilderKind {
    pub const ALL: [NodeBuilderKind; 2] = [NodeBuilderKind::Builtin, NodeBuilderKind::Zdbsp];

    pub fn label(self) -> &'static str {
        match self {
            NodeBuilderKind::Builtin => "Built-in (Rust)",
            NodeBuilderKind::Zdbsp => "zdbsp (external)",
        }
    }
}

impl std::fmt::Display for NodeBuilderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum View2DMode {
    Floor,
    Ceiling,
    Brightness,
    Wireframe,
}

impl Default for View2DMode {
    fn default() -> Self {
        View2DMode::Floor
    }
}

impl View2DMode {
    fn label(self) -> &'static str {
        match self {
            View2DMode::Floor => "Floor textures",
            View2DMode::Ceiling => "Ceiling textures",
            View2DMode::Brightness => "Brightness levels",
            View2DMode::Wireframe => "Wireframe",
        }
    }
}

/// Fixed grid sizes a user can cycle through (None = auto / zoom-derived).
const GRID_STEPS: &[Option<u32>] = &[
    None,
    Some(8),
    Some(16),
    Some(32),
    Some(64),
    Some(128),
    Some(256),
    Some(512),
];

const MAX_RECENT: usize = 8;

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
            snap_to_grid: true,
            grid_size: None,
            recent_files: Vec::new(),
            engine_path: None,
            iwad_path: None,
            theme: ThemeKind::default(),
            view_mode: View2DMode::default(),
            full_brightness: false,
            show_3d_overlay: true,
            node_builder: NodeBuilderKind::default(),
            zdbsp_path: None,
        }
    }
}

impl Settings {
    /// Materialize a `core::NodeBuilder` from the persisted choice. Falls back
    /// to Builtin if the user selected zdbsp without supplying a path.
    pub fn make_node_builder(&self) -> doombuilder_core::map::NodeBuilder {
        use doombuilder_core::map::NodeBuilder as Nb;
        match self.node_builder {
            NodeBuilderKind::Builtin => Nb::Builtin,
            NodeBuilderKind::Zdbsp => match &self.zdbsp_path {
                Some(p) => Nb::Zdbsp { exe: p.clone(), extra_args: Vec::new() },
                None => Nb::Builtin,
            },
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

    /// Move `path` to the front of `recent_files`, dedupe, and cap at MAX_RECENT.
    pub fn push_recent(&mut self, path: PathBuf) {
        self.recent_files.retain(|p| p != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(MAX_RECENT);
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
    SnapToGrid,
}

impl SettingKey {
    fn label(self) -> &'static str {
        match self {
            SettingKey::ShowTextures => "Show sector textures (flats)",
            SettingKey::ShowSprites => "Show thing sprites (vs colored placeholders)",
            SettingKey::ShowGrid => "Show grid",
            SettingKey::ShowThings => "Show things",
            SettingKey::AlwaysShowVertices => "Always show vertex dots",
            SettingKey::SnapToGrid => "Snap to grid (drawing + vertex drag)",
        }
    }

    fn get(self, s: &Settings) -> bool {
        match self {
            SettingKey::ShowTextures => s.show_textures,
            SettingKey::ShowSprites => s.show_sprites,
            SettingKey::ShowGrid => s.show_grid,
            SettingKey::ShowThings => s.show_things,
            SettingKey::AlwaysShowVertices => s.always_show_vertices,
            SettingKey::SnapToGrid => s.snap_to_grid,
        }
    }

    fn set(self, s: &mut Settings, v: bool) {
        match self {
            SettingKey::ShowTextures => s.show_textures = v,
            SettingKey::ShowSprites => s.show_sprites = v,
            SettingKey::ShowGrid => s.show_grid = v,
            SettingKey::ShowThings => s.show_things = v,
            SettingKey::AlwaysShowVertices => s.always_show_vertices = v,
            SettingKey::SnapToGrid => s.snap_to_grid = v,
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
    /// Drag-to-draw for shape tools (Rectangle / Ellipse / Grid). The
    /// drag's start point is the shape's first corner; drag end commits.
    ShapeDraw { origin: Vec2 },
}

impl Default for App {
    fn default() -> Self {
        let settings = Settings::load_or_default();
        palette::set_active(settings.theme.palette());
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
            settings,
            camera2d: Camera2D::default(),
            camera3d: Camera3D::default(),
            geometry3d: Arc::new(View3DGeometry::default()),
            cache2d: Arc::new(Cache::new()),
            hover: None,
            selection: Arc::new(HashSet::new()),
            drag_rect: None,
            active_drag: None,
            cursor_world: None,
            last_click: None,
            map_name_buffer: String::new(),
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
            go_to_coords_x: String::new(),
            go_to_coords_y: String::new(),
            tag_range_input: String::new(),
            show_highlights: true,
            space_held: false,
            visual_camera_start: None,
            clipboard: None,
            selection_groups: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    OpenWadRequested,
    LoadResourcesRequested,
    OpenRecent(usize),
    ResourcesFilePicked(Option<PathBuf>),
    ResourcesLoaded(Result<AssetSummary, String>),
    NewMap(MapFormat),
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
    /// Flip the setting's current value. For menu items where the dispatcher
    /// can't read state to compute the inverse.
    ToggleSetting(SettingKey),
    /// Flip 2D ↔ 3D Visual mode. Bound to Q (GZDB convention).
    ToggleVisualMode,
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
    CycleGridStep(i32),
    PanCamera { dx_units: i32, dy_units: i32, fast: bool },
    /// 3D-mode-only: change camera height. Positive = up.
    VerticalCamera { units: i32, fast: bool },
    /// Shift+WASD dispatch. Routes to 3D movement when in `Mode::View3D`,
    /// otherwise falls back to existing 2D-mode shortcuts so nothing
    /// regresses (Shift+A = AutoAlignY, Shift+D = MakeDoor).
    FlyMove(FlyDirection),
    FitToScreen,
    OpenGoToCoords,
    GoToCoordsXChanged(String),
    GoToCoordsYChanged(String),
    GoToCoordsSubmit,
    SnapSelectionToGrid,
    ToggleHighlights,
    PlaceVisualCamera,
    DrawingRemoveLast,
    SpaceHeld(bool),
    AdjustSectorBrightness(i32),
    AdjustSectorFloor(i32),
    AdjustSectorCeiling(i32),
    MakeBrightnessGradient,
    MakeFloorGradient,
    MakeCeilingGradient,
    JoinSectors,
    MergeSectors,
    MakeDoor,
    StartRectangleDraw,
    StartEllipseDraw,
    StartCurveDraw,
    StartGridDraw,
    CurveSelectedLines,
    /// +/- while in a shape-draw tool adjusts subdivisions / bevel / cols.
    AdjustDrawParam(i32),
    OpenMapStats,
    SetView2DMode(View2DMode),
    ToggleFullBrightness,
    Toggle3DOverlay,
    FlipSidedefs,
    AlignLinedefs,
    StitchLines,
    AlignThingsToNearestLine,
    PointThingsToCursor,
    OpenMapAnalysis,
    OpenUsedTags,
    OpenTagRange,
    OpenThingTypes,
    OpenMapInWad,
    OpenMapOptions,
    OpenAboutDialog,
    OpenConfigFolder,
    ReloadResources,
    MapNameInputChanged(String),
    MapNameSubmit,
    TagRangeInputChanged(String),
    TagRangeApply,
    TestMapAtCursor,
    CopySelection,
    CutSelection,
    PasteSelection,
    PasteProperties,
    AssignGroup(usize),
    SelectGroup(usize),
    RotateSelection90,
    FlipSelectionHorizontal,
    FlipSelectionVertical,
    AutoAlignX,
    AutoAlignY,
    AutoAlignBoth,
    /// Multipurpose G hotkey. The handler routes to gradient/grid-cycle based
    /// on edit mode and selection.
    GHotkey { shift: bool, ctrl: bool },
    SetTheme(ThemeKind),
    TestMap,
    PickEngineRequested,
    EnginePathPicked(Option<PathBuf>),
    PickIwadRequested,
    IwadPathPicked(Option<PathBuf>),
    SetNodeBuilder(NodeBuilderKind),
    PickZdbspRequested,
    ZdbspPathPicked(Option<PathBuf>),
    MakeSector,
    SplitLines,
    MergeVertices,
    FlipLines,
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

#[derive(Debug, Clone, Copy)]
enum SelectionTransform {
    Rotate90,
    FlipH,
    FlipV,
}

#[derive(Debug, Clone, Copy)]
pub enum FlyDirection {
    Forward,
    Back,
    StrafeLeft,
    StrafeRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IssueSeverity {
    Error,
    Warning,
}

impl IssueSeverity {
    fn label(self) -> &'static str {
        match self {
            IssueSeverity::Error => "ERROR",
            IssueSeverity::Warning => "WARN",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MapIssue {
    pub severity: IssueSeverity,
    pub category: &'static str,
    pub message: String,
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
        let p = self.settings.theme.palette();
        Theme::custom(
            self.settings.theme.label().to_string(),
            iced::theme::Palette {
                background: p.primary,
                text: p.text,
                primary: p.secondary,
                success: p.secondary,
                warning: p.danger,
                danger: p.danger,
            },
        )
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
            Message::LoadResourcesRequested => {
                self.status = "Choose an IWAD/PWAD to load textures + sprites from...".into();
                Task::perform(pick_file(), Message::ResourcesFilePicked)
            }
            Message::OpenRecent(idx) => {
                let Some(path) = self.settings.recent_files.get(idx).cloned() else {
                    return Task::none();
                };
                self.status = format!("Loading {}...", path.display());
                Task::perform(load_asset(path), Message::AssetLoaded)
            }
            Message::ResourcesFilePicked(None) => {
                self.status = "Resource load cancelled.".into();
                Task::none()
            }
            Message::ResourcesFilePicked(Some(path)) => {
                self.status = format!("Loading resources from {}...", path.display());
                Task::perform(load_asset(path), Message::ResourcesLoaded)
            }
            Message::ResourcesLoaded(Ok(asset)) => {
                self.settings.push_recent(asset.path.clone());
                self.persist_settings();
                // Merge textures/sprites only; preserve current map, wad list,
                // and selection. Lets the user keep a new-map-from-scratch
                // session and still pick floor/wall textures.
                let mut sorted: Vec<String> =
                    asset.texture_handles.keys().cloned().collect();
                sorted.sort();
                self.sorted_texture_names = Arc::new(sorted);
                self.texture_handles = asset.texture_handles;
                self.sprite_handles = asset.sprite_handles;
                self.sprite_dims = asset.sprite_dims;
                self.textures = asset.textures;
                // Re-rasterise sector fills with the freshly-loaded textures.
                self.rebuild_sector_fills();
                self.cache2d.clear();
                self.status = format!("Loaded resources from {}", asset.path.display());
                Task::none()
            }
            Message::ResourcesLoaded(Err(err)) => {
                self.status = format!("Resource load failed: {err}");
                Task::none()
            }
            Message::NewMap(format) => {
                self.do_new_map(format);
                Task::none()
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
                let builder = self.settings.make_node_builder();
                Task::perform(save_map_to_path(map, path, builder), Message::SaveMapDone)
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
                // Auto-pick the matching game config so action/thing/sector
                // pickers show the right catalog without the user having to
                // touch the dropdown. User-overridable: switching the
                // dropdown after load always wins.
                if let Some(wad) = asset.wad.as_ref() {
                    let detected = GameConfig::detect_for_wad(wad);
                    if detected != self.current_config_name {
                        if let Some(cfg) = GameConfig::builtin(detected) {
                            self.config = Arc::new(cfg);
                            self.current_config_name = detected.to_string();
                        }
                    }
                }
                self.status = format!("Loaded {} ({})", asset.path.display(), self.current_config_name);
                self.settings.push_recent(asset.path.clone());
                self.persist_settings();
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
                // Close the modal map picker if it was the source of this
                // selection; harmless when MapSelected came from the toolbar.
                if matches!(self.active_picker, Some(ActivePicker::MapInWad)) {
                    self.active_picker = None;
                }
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
                if mode == Mode::View3D {
                    if let Some(pos) = self.visual_camera_start {
                        self.camera3d.target.x = pos.x;
                        self.camera3d.target.y = pos.y;
                    }
                }
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
            Message::ToggleSetting(key) => {
                let new = !key.get(&self.settings);
                key.set(&mut self.settings, new);
                self.persist_settings();
                self.cache2d.clear();
                self.status = format!("{}: {}", key.label(), if new { "on" } else { "off" });
                Task::none()
            }
            Message::ToggleVisualMode => {
                let next = if matches!(self.mode, Mode::View3D) {
                    Mode::View2D
                } else {
                    Mode::View3D
                };
                // Reuse the existing Mode handler so all of its side effects
                // (camera placement, cache invalidation, etc.) fire too.
                return self.handle_message(Message::Mode(next));
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
                self.delete_selection();
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
            Message::CycleGridStep(delta) => {
                self.cycle_grid_step(delta);
                Task::none()
            }
            Message::FitToScreen => {
                if let Some(map) = self.map.as_ref() {
                    if let Some((min, max)) = map_aabb(map) {
                        // Use a generous viewport size so we get a useful zoom
                        // without needing to know the on-screen bounds here.
                        self.camera2d.frame_aabb(min, max, Vec2::new(1200.0, 800.0));
                        self.cache2d.clear();
                        self.status = "Framed map.".into();
                    }
                }
                Task::none()
            }
            Message::OpenGoToCoords => {
                self.go_to_coords_x = format!("{:.0}", self.camera2d.center.x);
                self.go_to_coords_y = format!("{:.0}", self.camera2d.center.y);
                self.active_picker = Some(ActivePicker::GoToCoords);
                Task::none()
            }
            Message::GoToCoordsXChanged(s) => {
                self.go_to_coords_x = s;
                Task::none()
            }
            Message::GoToCoordsYChanged(s) => {
                self.go_to_coords_y = s;
                Task::none()
            }
            Message::GoToCoordsSubmit => {
                let x = self.go_to_coords_x.trim().parse::<f32>().ok();
                let y = self.go_to_coords_y.trim().parse::<f32>().ok();
                if let (Some(x), Some(y)) = (x, y) {
                    self.camera2d.center = Vec2::new(x, y);
                    self.active_picker = None;
                    self.cache2d.clear();
                    self.status = format!("Camera centered at ({x:.0}, {y:.0})");
                } else {
                    self.status = "Go To: both X and Y must be numbers.".into();
                }
                Task::none()
            }
            Message::SnapSelectionToGrid => {
                self.snap_selection_to_grid();
                Task::none()
            }
            Message::ToggleHighlights => {
                self.show_highlights = !self.show_highlights;
                if !self.show_highlights {
                    self.hover = None;
                }
                self.cache2d.clear();
                self.status = if self.show_highlights {
                    "Highlights on".into()
                } else {
                    "Highlights off".into()
                };
                Task::none()
            }
            Message::PlaceVisualCamera => {
                if let Some(world) = self.cursor_world {
                    self.visual_camera_start = Some(world);
                    self.status = format!(
                        "Visual camera placed at ({:.0}, {:.0}).",
                        world.x, world.y
                    );
                } else {
                    self.status = "Visual camera: move cursor over canvas first.".into();
                }
                Task::none()
            }
            Message::DrawingRemoveLast => {
                if self.drawing.is_some() {
                    self.drawing_remove_last();
                } else {
                    // Backspace outside of drawing mode falls back to the
                    // classic "delete selection" behavior so muscle memory
                    // still works.
                    self.delete_selection();
                }
                Task::none()
            }
            Message::SpaceHeld(down) => {
                self.space_held = down;
                Task::none()
            }
            Message::AdjustSectorBrightness(delta) => {
                let sel = self.selected_sectors();
                self.adjust_sectors_int_field(&sel, SectorIntField::Light, delta);
                Task::none()
            }
            Message::AdjustSectorFloor(delta) => {
                let sel = self.selected_sectors();
                self.adjust_sectors_int_field(&sel, SectorIntField::FloorHeight, delta);
                Task::none()
            }
            Message::AdjustSectorCeiling(delta) => {
                let sel = self.selected_sectors();
                self.adjust_sectors_int_field(&sel, SectorIntField::CeilingHeight, delta);
                Task::none()
            }
            Message::MakeBrightnessGradient => {
                self.make_sector_gradient(SectorIntField::Light);
                Task::none()
            }
            Message::MakeFloorGradient => {
                self.make_sector_gradient(SectorIntField::FloorHeight);
                Task::none()
            }
            Message::MakeCeilingGradient => {
                self.make_sector_gradient(SectorIntField::CeilingHeight);
                Task::none()
            }
            Message::JoinSectors => {
                self.do_join_sectors(false);
                Task::none()
            }
            Message::MergeSectors => {
                self.do_join_sectors(true);
                Task::none()
            }
            Message::MakeDoor => {
                self.do_make_door();
                Task::none()
            }
            Message::StartRectangleDraw => {
                self.start_shape_draw(DrawTool::Rectangle { origin: None, bevel: 0 });
                Task::none()
            }
            Message::StartEllipseDraw => {
                self.start_shape_draw(DrawTool::Ellipse {
                    origin: None,
                    subdivisions: 16,
                });
                Task::none()
            }
            Message::StartCurveDraw => {
                self.start_shape_draw(DrawTool::Curve {
                    points: Vec::new(),
                    subdivisions: 12,
                });
                Task::none()
            }
            Message::StartGridDraw => {
                self.start_shape_draw(DrawTool::Grid {
                    origin: None,
                    cols: 4,
                    rows: 4,
                });
                Task::none()
            }
            Message::CurveSelectedLines => {
                self.curve_selected_lines();
                Task::none()
            }
            Message::AdjustDrawParam(delta) => {
                self.adjust_draw_param(delta);
                Task::none()
            }
            Message::OpenMapStats => {
                self.active_picker = Some(ActivePicker::MapStats);
                Task::none()
            }
            Message::SetView2DMode(mode) => {
                self.settings.view_mode = mode;
                self.persist_settings();
                self.rebuild_sector_fills();
                self.cache2d.clear();
                self.status = format!("View: {}", mode.label());
                Task::none()
            }
            Message::ToggleFullBrightness => {
                self.settings.full_brightness = !self.settings.full_brightness;
                self.persist_settings();
                self.rebuild_geometry3d();
                self.status = if self.settings.full_brightness {
                    "Full brightness on".into()
                } else {
                    "Full brightness off".into()
                };
                Task::none()
            }
            Message::Toggle3DOverlay => {
                self.settings.show_3d_overlay = !self.settings.show_3d_overlay;
                self.persist_settings();
                self.status = if self.settings.show_3d_overlay {
                    "3D preview on".into()
                } else {
                    "3D preview off".into()
                };
                Task::none()
            }
            Message::FlipSidedefs => {
                self.do_flip_sidedefs();
                Task::none()
            }
            Message::AlignThingsToNearestLine => {
                self.do_align_things_to_lines();
                Task::none()
            }
            Message::PointThingsToCursor => {
                self.do_point_things_to_cursor();
                Task::none()
            }
            Message::OpenMapAnalysis => {
                self.active_picker = Some(ActivePicker::MapAnalysis);
                Task::none()
            }
            Message::OpenUsedTags => {
                self.active_picker = Some(ActivePicker::UsedTags);
                Task::none()
            }
            Message::OpenTagRange => {
                self.tag_range_input = "1".into();
                self.active_picker = Some(ActivePicker::TagRange);
                Task::none()
            }
            Message::OpenThingTypes => {
                self.active_picker = Some(ActivePicker::ThingTypes);
                Task::none()
            }
            Message::OpenMapInWad => {
                if self.maps.is_empty() {
                    self.status = "No WAD loaded — open a WAD first.".into();
                } else {
                    self.active_picker = Some(ActivePicker::MapInWad);
                    self.picker_filter.clear();
                }
                Task::none()
            }
            Message::OpenAboutDialog => {
                self.active_picker = Some(ActivePicker::About);
                Task::none()
            }
            Message::OpenConfigFolder => {
                if let Some(folder) = Settings::config_path().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
                    let _ = std::fs::create_dir_all(&folder);
                    let opener = if cfg!(target_os = "macos") {
                        "open"
                    } else if cfg!(target_os = "windows") {
                        "explorer"
                    } else {
                        "xdg-open"
                    };
                    match std::process::Command::new(opener).arg(&folder).spawn() {
                        Ok(_) => self.status = format!("Opened {}", folder.display()),
                        Err(e) => self.status = format!("Open config folder failed: {e}"),
                    }
                } else {
                    self.status = "No config folder available.".into();
                }
                Task::none()
            }
            Message::ReloadResources => {
                let Some(path) = self.wad_path.clone() else {
                    self.status = "No WAD loaded — nothing to reload.".into();
                    return Task::none();
                };
                self.status = format!("Reloading {}...", path.display());
                Task::perform(load_asset(path), Message::AssetLoaded)
            }
            Message::OpenMapOptions => {
                let Some(map) = self.map.as_ref() else {
                    self.status = "No map loaded.".into();
                    return Task::none();
                };
                self.map_name_buffer = map.name.clone();
                self.active_picker = Some(ActivePicker::MapOptions);
                Task::none()
            }
            Message::MapNameInputChanged(s) => {
                // Doom map markers are 8-byte uppercase ASCII. Coerce as
                // the user types so they can't end up with a name the WAD
                // writer would silently truncate or reject.
                let mut clean: String = s
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .map(|c| c.to_ascii_uppercase())
                    .collect();
                clean.truncate(8);
                self.map_name_buffer = clean;
                Task::none()
            }
            Message::MapNameSubmit => {
                let name = self.map_name_buffer.trim().to_string();
                if name.is_empty() {
                    self.status = "Map name can't be empty.".into();
                    return Task::none();
                }
                if let Some(map) = self.map.as_mut() {
                    let map_mut = Arc::make_mut(map);
                    map_mut.name = name.clone();
                    self.selected_map = Some(name.clone());
                    self.status = format!("Map renamed to {name}.");
                }
                self.active_picker = None;
                Task::none()
            }
            Message::TagRangeInputChanged(s) => {
                self.tag_range_input = s;
                Task::none()
            }
            Message::TagRangeApply => {
                self.apply_tag_range();
                Task::none()
            }
            Message::TestMapAtCursor => {
                self.test_map_at_cursor();
                Task::none()
            }
            Message::CopySelection => {
                self.copy_selection();
                Task::none()
            }
            Message::CutSelection => {
                self.copy_selection();
                self.delete_selection();
                self.status = "Cut selection to clipboard.".into();
                Task::none()
            }
            Message::PasteSelection => {
                self.paste_selection();
                Task::none()
            }
            Message::PasteProperties => {
                self.paste_properties();
                Task::none()
            }
            Message::AssignGroup(idx) => {
                if idx < 10 {
                    self.selection_groups[idx] = Some((*self.selection).clone());
                    self.status = format!(
                        "Stored selection in group {idx} ({} item(s)).",
                        self.selection.len()
                    );
                }
                Task::none()
            }
            Message::SelectGroup(idx) => {
                if let Some(group) = self.selection_groups.get(idx).and_then(|g| g.clone()) {
                    self.selection = Arc::new(group);
                    self.cache2d.clear();
                    self.status = format!(
                        "Recalled group {idx} ({} item(s)).",
                        self.selection.len()
                    );
                } else {
                    self.status = format!("Group {idx} is empty.");
                }
                Task::none()
            }
            Message::RotateSelection90 => {
                self.transform_selection(SelectionTransform::Rotate90);
                Task::none()
            }
            Message::FlipSelectionHorizontal => {
                self.transform_selection(SelectionTransform::FlipH);
                Task::none()
            }
            Message::FlipSelectionVertical => {
                self.transform_selection(SelectionTransform::FlipV);
                Task::none()
            }
            Message::AlignLinedefs => {
                self.do_align_linedefs();
                Task::none()
            }
            Message::StitchLines => {
                self.do_stitch_lines();
                Task::none()
            }
            Message::AutoAlignX => {
                self.do_auto_align(doombuilder_core::edit::AutoAlignAxis::X);
                Task::none()
            }
            Message::AutoAlignY => {
                self.do_auto_align(doombuilder_core::edit::AutoAlignAxis::Y);
                Task::none()
            }
            Message::AutoAlignBoth => {
                self.do_auto_align(doombuilder_core::edit::AutoAlignAxis::Both);
                Task::none()
            }
            Message::GHotkey { shift, ctrl } => {
                let has_sector_sel = self.edit_mode == EditMode::Sectors
                    && self.selection.iter().any(|h| matches!(h, HighlightKind::Sector(_)));
                if has_sector_sel {
                    if ctrl {
                        self.make_sector_gradient(SectorIntField::FloorHeight);
                    } else if shift {
                        self.make_sector_gradient(SectorIntField::CeilingHeight);
                    } else {
                        self.make_sector_gradient(SectorIntField::Light);
                    }
                } else {
                    // Fall back to grid step cycling so the key isn't dead in
                    // other modes.
                    self.cycle_grid_step(if shift { -1 } else { 1 });
                }
                Task::none()
            }
            Message::FlyMove(dir) => {
                if self.mode == Mode::View3D {
                    let step = 96.0;
                    let yaw = self.camera3d.yaw;
                    let fwd = Vec2::new(yaw.cos(), yaw.sin());
                    let right = Vec2::new(fwd.y, -fwd.x);
                    let delta = match dir {
                        FlyDirection::Forward => fwd * step,
                        FlyDirection::Back => fwd * -step,
                        FlyDirection::StrafeLeft => right * -step,
                        FlyDirection::StrafeRight => right * step,
                    };
                    self.camera3d.target.x += delta.x;
                    self.camera3d.target.y += delta.y;
                } else {
                    // Preserve existing 2D-mode Shift+letter bindings.
                    match dir {
                        FlyDirection::StrafeLeft => return self.handle_message(Message::AutoAlignY),
                        FlyDirection::StrafeRight => return self.handle_message(Message::MakeDoor),
                        _ => {}
                    }
                }
                Task::none()
            }
            Message::VerticalCamera { units, fast } => {
                if self.mode == Mode::View3D {
                    let step = 32.0 * if fast { 4.0 } else { 1.0 };
                    self.camera3d.target.z += units as f32 * step;
                }
                Task::none()
            }
            Message::PanCamera { dx_units, dy_units, fast } => {
                let step = self.effective_grid_step().max(8.0);
                let mul = if fast { 4.0 } else { 1.0 };
                let dx = dx_units as f32 * step * mul;
                let dy = dy_units as f32 * step * mul;
                match self.mode {
                    Mode::View2D => {
                        self.camera2d.center += Vec2::new(dx, dy);
                        self.cache2d.clear();
                    }
                    Mode::View3D => {
                        // Slide the orbit target along the camera's flat yaw
                        // frame so arrows feel like forward/strafe movement.
                        let yaw = self.camera3d.yaw;
                        let fwd = Vec2::new(yaw.cos(), yaw.sin());
                        let right = Vec2::new(fwd.y, -fwd.x);
                        let delta = fwd * dy + right * dx;
                        self.camera3d.target.x += delta.x;
                        self.camera3d.target.y += delta.y;
                    }
                }
                Task::none()
            }
            Message::SetTheme(t) => {
                self.settings.theme = t;
                palette::set_active(t.palette());
                self.persist_settings();
                self.cache2d.clear();
                self.status = format!("Theme: {}", t.label());
                Task::none()
            }
            Message::TestMap => {
                self.test_map();
                Task::none()
            }
            Message::PickEngineRequested => {
                Task::perform(pick_executable(), Message::EnginePathPicked)
            }
            Message::EnginePathPicked(None) => Task::none(),
            Message::EnginePathPicked(Some(path)) => {
                self.settings.engine_path = Some(path.clone());
                self.persist_settings();
                self.status = format!("Engine set: {}", path.display());
                Task::none()
            }
            Message::PickIwadRequested => Task::perform(pick_file(), Message::IwadPathPicked),
            Message::IwadPathPicked(None) => Task::none(),
            Message::IwadPathPicked(Some(path)) => {
                self.settings.iwad_path = Some(path.clone());
                self.persist_settings();
                self.status = format!("IWAD set: {}", path.display());
                Task::none()
            }
            Message::SetNodeBuilder(kind) => {
                self.settings.node_builder = kind;
                self.persist_settings();
                self.status = format!("Node builder: {kind}");
                Task::none()
            }
            Message::PickZdbspRequested => {
                Task::perform(pick_executable(), Message::ZdbspPathPicked)
            }
            Message::ZdbspPathPicked(None) => Task::none(),
            Message::ZdbspPathPicked(Some(path)) => {
                self.settings.zdbsp_path = Some(path.clone());
                self.persist_settings();
                self.status = format!("zdbsp set: {}", path.display());
                Task::none()
            }
            Message::MakeSector => {
                self.do_make_sector();
                Task::none()
            }
            Message::SplitLines => {
                self.do_split_lines();
                Task::none()
            }
            Message::MergeVertices => {
                self.do_merge_vertices();
                Task::none()
            }
            Message::FlipLines => {
                self.do_flip_lines();
                Task::none()
            }
            Message::Quit => iced::exit(),
            Message::Noop => Task::none(),
        }
    }

    /// Discard any loaded map and start a brand-new empty map of the given
    /// format. The user can save it with any name via Save As (the default
    /// name is `MAP01`). No WAD context is required; this also clears any
    /// previously selected map so the inspector starts fresh.
    fn do_new_map(&mut self, format: MapFormat) {
        self.reset_map_state();
        let mut map = doombuilder_core::map::Map::new("MAP01", format);
        // Auto-insert a Player 1 start at the origin so a freshly saved WAD
        // loads in an engine without an immediate "no player start" error.
        // Kind 1 = Player 1 start; flags 7 = easy+medium+hard.
        map.things.insert(doombuilder_core::map::MapThing {
            x: 0,
            y: 0,
            angle: 0,
            kind: 1,
            flags: 7,
            tid: 0,
            z: 0,
            special: 0,
            args: [0; 5],
        });
        self.map_stats = Some(MapStats {
            name: map.name.clone(),
            format: map.format,
            vertices: 0,
            linedefs: 0,
            sidedefs: 0,
            sectors: 0,
            things: map.things.len(),
        });
        self.selected_map = Some(map.name.clone());
        self.map = Some(Arc::new(map));
        // Empty derived caches; geometry rebuild will populate them as the
        // user draws. Frame a 1024-unit window so they have visible grid.
        self.rebuild_geometry_indices();
        self.camera2d
            .frame_aabb(Vec2::new(-512.0, -512.0), Vec2::new(512.0, 512.0), Vec2::new(800.0, 600.0));
        let label = match format {
            MapFormat::Doom => "Doom",
            MapFormat::Hexen => "Hexen",
        };
        self.status = format!("New {label}-format map (MAP01). Press D to draw.");
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
                keyboard::Key::Named(keyboard::key::Named::ArrowLeft) => {
                    Message::PanCamera { dx_units: -1, dy_units: 0, fast: modifiers.shift() }
                }
                keyboard::Key::Named(keyboard::key::Named::ArrowRight) => {
                    Message::PanCamera { dx_units: 1, dy_units: 0, fast: modifiers.shift() }
                }
                keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                    Message::PanCamera { dx_units: 0, dy_units: 1, fast: modifiers.shift() }
                }
                keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                    Message::PanCamera { dx_units: 0, dy_units: -1, fast: modifiers.shift() }
                }
                keyboard::Key::Character("a") if modifiers.command() && !modifiers.shift() => {
                    Message::SelectAll
                }
                keyboard::Key::Character("z") if modifiers.command() && modifiers.shift() => {
                    Message::Redo
                }
                keyboard::Key::Character("z") if modifiers.command() => Message::Undo,
                keyboard::Key::Character("y") if modifiers.command() => Message::Redo,
                keyboard::Key::Character("s") if modifiers.command() => Message::SaveMapRequested,
                keyboard::Key::Named(keyboard::key::Named::Delete) => Message::DeleteSelection,
                keyboard::Key::Named(keyboard::key::Named::Backspace) => {
                    // Routed in the handler: removes last drawn vertex when
                    // drawing is active, otherwise deletes the selection.
                    Message::DrawingRemoveLast
                }
                keyboard::Key::Named(keyboard::key::Named::Home) => Message::FitToScreen,
                keyboard::Key::Named(keyboard::key::Named::F2) => Message::OpenMapOptions,
                keyboard::Key::Character("g") if modifiers.command() && modifiers.shift() => {
                    Message::OpenGoToCoords
                }
                keyboard::Key::Character("g") if modifiers.command() => {
                    Message::GHotkey { shift: false, ctrl: true }
                }
                // Alt+G — toggle grid rendering (matches GZDoom Builder).
                keyboard::Key::Character("g") if modifiers.alt() => {
                    Message::ToggleSetting(SettingKey::ShowGrid)
                }
                keyboard::Key::Character("[") => Message::CycleGridStep(1),
                keyboard::Key::Character("]") => Message::CycleGridStep(-1),
                keyboard::Key::Character("h") if !modifiers.command() => Message::ToggleHighlights,
                keyboard::Key::Character("b") if !modifiers.command() => Message::ToggleFullBrightness,
                keyboard::Key::Character("w") if modifiers.command() => Message::PlaceVisualCamera,
                keyboard::Key::Named(keyboard::key::Named::Space) => Message::SpaceHeld(true),
                keyboard::Key::Named(keyboard::key::Named::Insert) => Message::InsertThing,
                keyboard::Key::Character("i") if !modifiers.command() => Message::InsertThing,
                keyboard::Key::Character("d") if modifiers.command() && modifiers.shift() => {
                    Message::StartRectangleDraw
                }
                keyboard::Key::Character("d") if modifiers.alt() && modifiers.shift() => {
                    Message::StartEllipseDraw
                }
                keyboard::Key::Character("d") if modifiers.command() && modifiers.alt() => {
                    Message::StartCurveDraw
                }
                keyboard::Key::Character("d") if modifiers.shift() => Message::MakeDoor,
                keyboard::Key::Character("d") if !modifiers.command() => Message::ToggleDrawing,
                // Shift+WASD fly-cam (3D mode); falls back to existing 2D
                // bindings inside the handler when not in 3D.
                keyboard::Key::Character("w") if modifiers.shift() && !modifiers.command() => {
                    Message::FlyMove(FlyDirection::Forward)
                }
                keyboard::Key::Character("a") if modifiers.shift() && !modifiers.command() => {
                    Message::FlyMove(FlyDirection::StrafeLeft)
                }
                keyboard::Key::Character("s") if modifiers.shift() && !modifiers.command() => {
                    Message::FlyMove(FlyDirection::Back)
                }
                keyboard::Key::Character("d") if modifiers.shift() && !modifiers.command() => {
                    Message::FlyMove(FlyDirection::StrafeRight)
                }
                keyboard::Key::Character("c") if modifiers.command() && modifiers.shift() => {
                    Message::PasteProperties
                }
                keyboard::Key::Character("c") if modifiers.command() => Message::CopySelection,
                keyboard::Key::Character("x") if modifiers.command() => Message::CutSelection,
                keyboard::Key::Character("v") if modifiers.command() && modifiers.shift() => {
                    Message::PasteProperties
                }
                keyboard::Key::Character("v") if modifiers.command() => Message::PasteSelection,
                keyboard::Key::Character("e") if modifiers.shift() => Message::FlipSelectionVertical,
                keyboard::Key::Character("e") if modifiers.command() => {
                    Message::FlipSelectionHorizontal
                }
                keyboard::Key::Character("e") if !modifiers.command() && !modifiers.shift() => {
                    Message::RotateSelection90
                }
                keyboard::Key::Character("c") if modifiers.shift() => Message::CurveSelectedLines,
                keyboard::Key::Character("f") if modifiers.shift() => Message::FlipSidedefs,
                keyboard::Key::Character("l") if modifiers.shift() => Message::PointThingsToCursor,
                // Auto-align: A = X, Shift+A = Y, Cmd+Shift+A = X+Y.
                // (Plain Cmd+A is already "Select All" above.)
                keyboard::Key::Character("a") if modifiers.command() && modifiers.shift() => {
                    Message::AutoAlignBoth
                }
                keyboard::Key::Character("a") if modifiers.shift() && !modifiers.command() => {
                    Message::AutoAlignY
                }
                keyboard::Key::Character("a")
                    if !modifiers.command() && !modifiers.shift() && !modifiers.alt() =>
                {
                    Message::AutoAlignX
                }
                keyboard::Key::Character("=") | keyboard::Key::Character("+") => {
                    Message::AdjustDrawParam(1)
                }
                keyboard::Key::Character("-") | keyboard::Key::Character("_") => {
                    Message::AdjustDrawParam(-1)
                }
                keyboard::Key::Character("m") if modifiers.command() => Message::MakeSector,
                keyboard::Key::Character("j") if modifiers.shift() => Message::MergeSectors,
                keyboard::Key::Character("j") if !modifiers.command() => Message::JoinSectors,
                // `g` is context-sensitive: gradients in Sectors mode, grid
                // cycling otherwise. Decision lives in the handler.
                keyboard::Key::Character("g") if !modifiers.command() => {
                    Message::GHotkey {
                        shift: modifiers.shift(),
                        ctrl: false,
                    }
                }
                keyboard::Key::Named(keyboard::key::Named::F5) => Message::TestMap,
                keyboard::Key::Named(keyboard::key::Named::F9) if modifiers.command() => {
                    Message::TestMapAtCursor
                }
                keyboard::Key::Named(keyboard::key::Named::F4) => Message::OpenMapAnalysis,
                keyboard::Key::Named(keyboard::key::Named::F11) => Message::OpenMapAnalysis,
                keyboard::Key::Named(keyboard::key::Named::PageUp) => Message::VerticalCamera {
                    units: 1,
                    fast: modifiers.shift(),
                },
                keyboard::Key::Named(keyboard::key::Named::PageDown) => Message::VerticalCamera {
                    units: -1,
                    fast: modifiers.shift(),
                },
                // Number keys 0..9 are reassigned to selection groups:
                //   plain N   → recall group N
                //   Cmd+N     → store current selection into group N
                // Edit-mode hotkeys live on V / L / S / T (see below).
                keyboard::Key::Character("0") if modifiers.command() => Message::AssignGroup(0),
                keyboard::Key::Character("1") if modifiers.command() => Message::AssignGroup(1),
                keyboard::Key::Character("2") if modifiers.command() => Message::AssignGroup(2),
                keyboard::Key::Character("3") if modifiers.command() => Message::AssignGroup(3),
                keyboard::Key::Character("4") if modifiers.command() => Message::AssignGroup(4),
                keyboard::Key::Character("5") if modifiers.command() => Message::AssignGroup(5),
                keyboard::Key::Character("6") if modifiers.command() => Message::AssignGroup(6),
                keyboard::Key::Character("7") if modifiers.command() => Message::AssignGroup(7),
                keyboard::Key::Character("8") if modifiers.command() => Message::AssignGroup(8),
                keyboard::Key::Character("9") if modifiers.command() => Message::AssignGroup(9),
                keyboard::Key::Character("0") if !modifiers.command() => Message::SelectGroup(0),
                keyboard::Key::Character("1") if !modifiers.command() => Message::SelectGroup(1),
                keyboard::Key::Character("2") if !modifiers.command() => Message::SelectGroup(2),
                keyboard::Key::Character("3") if !modifiers.command() => Message::SelectGroup(3),
                keyboard::Key::Character("4") if !modifiers.command() => Message::SelectGroup(4),
                keyboard::Key::Character("5") if !modifiers.command() => Message::SelectGroup(5),
                keyboard::Key::Character("6") if !modifiers.command() => Message::SelectGroup(6),
                keyboard::Key::Character("7") if !modifiers.command() => Message::SelectGroup(7),
                keyboard::Key::Character("8") if !modifiers.command() => Message::SelectGroup(8),
                keyboard::Key::Character("9") if !modifiers.command() => Message::SelectGroup(9),
                // Q toggles between 2D and 3D Visual Mode (GZDB convention).
                keyboard::Key::Character("q") if !modifiers.command() => {
                    Message::ToggleVisualMode
                }
                keyboard::Key::Character("v") if !modifiers.command() => {
                    Message::SetEditMode(EditMode::Vertices)
                }
                keyboard::Key::Character("l") if !modifiers.command() => {
                    Message::SetEditMode(EditMode::Linedefs)
                }
                keyboard::Key::Character("s") if !modifiers.command() => {
                    Message::SetEditMode(EditMode::Sectors)
                }
                keyboard::Key::Character("t") if !modifiers.command() => {
                    Message::SetEditMode(EditMode::Things)
                }
                _ => Message::ModifiersChanged(modifiers),
            },
            keyboard::Event::KeyReleased { key, modifiers, .. } => match key.as_ref() {
                keyboard::Key::Named(keyboard::key::Named::Space) => Message::SpaceHeld(false),
                _ => Message::ModifiersChanged(modifiers),
            },
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
        let Some(map) = &self.map else {
            self.geometry3d = Arc::new(View3DGeometry::default());
            return;
        };
        // Without a Resource WAD loaded we still want the 3D view to show
        // untextured geometry, so fall back to an empty TextureSet rather
        // than rendering a black void.
        let empty;
        let textures: &doombuilder_core::textures::TextureSet = match &self.textures {
            Some(t) => t.as_ref(),
            None => {
                empty = doombuilder_core::textures::TextureSet::empty(Vec::new());
                &empty
            }
        };
        let geom = build_geometry(
            map,
            &self.sector_meshes,
            &self.walls,
            textures,
            self.spatial.as_deref(),
            &self.config,
            self.settings.full_brightness,
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
            View2DMessage::Wheel { units, pivot, viewport } => {
                self.handle_wheel(units, pivot, viewport);
            }
            View2DMessage::HoverAt(world) => {
                self.cursor_world = Some(world);
                let new_hover = if self.show_highlights {
                    self.hit_test(world)
                } else {
                    None
                };
                if new_hover != self.hover {
                    self.hover = new_hover;
                }
            }
            View2DMessage::HoverCleared => {
                self.hover = None;
                self.cursor_world = None;
            }
            View2DMessage::ClickAt(world) => {
                // Detect double-click: two consecutive ClickAt events within
                // 400ms and ~6 px (in world units, scaled by zoom). Threshold
                // matches macOS's default and feels right at every zoom level.
                let now = std::time::Instant::now();
                let px_world = (6.0_f32 / self.camera2d.zoom.max(1e-6)).max(2.0);
                let is_double = self
                    .last_click
                    .as_ref()
                    .map(|(t, p)| {
                        now.duration_since(*t).as_millis() <= 400
                            && (p.x - world.x).abs() <= px_world
                            && (p.y - world.y).abs() <= px_world
                    })
                    .unwrap_or(false);
                self.last_click = if is_double { None } else { Some((now, world)) };

                if self.drawing.is_some() {
                    self.drawing_click(world);
                } else {
                    let hit = self.hit_test(world);
                    // Things mode + double-click on empty space → place a
                    // default thing and immediately open the kind picker so
                    // the user can pick what they actually wanted. Cancelling
                    // the picker leaves the placeholder; they can delete or
                    // re-pick. Cheaper UX than a separate "type then place"
                    // gesture and matches how vertex/sector specials work.
                    if is_double
                        && hit.is_none()
                        && self.edit_mode == EditMode::Things
                    {
                        self.insert_thing_and_open_picker(world);
                        return;
                    }
                    // Vertex mode + click on empty space near a linedef =>
                    // insert a vertex on that linedef. Falls through to normal
                    // selection behavior on any miss.
                    if hit.is_none()
                        && self.edit_mode == EditMode::Vertices
                        && !self.modifiers.shift()
                    {
                        if self.try_insert_vertex_on_line(world) {
                            return;
                        }
                    }
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

    /// Active grid spacing in world units: respects the fixed-size override
    /// in settings, falling back to the camera's zoom-derived auto step.
    fn effective_grid_step(&self) -> f32 {
        match self.settings.grid_size {
            Some(n) if n > 0 => n as f32,
            _ => self.camera2d.grid_step(),
        }
    }

    /// Cycle the fixed grid size by `delta` positions through `GRID_STEPS`.
    fn cycle_grid_step(&mut self, delta: i32) {
        let current = GRID_STEPS
            .iter()
            .position(|s| *s == self.settings.grid_size)
            .unwrap_or(0);
        let n = GRID_STEPS.len() as i32;
        let idx = ((current as i32 + delta).rem_euclid(n)) as usize;
        self.settings.grid_size = GRID_STEPS[idx];
        self.persist_settings();
        self.cache2d.clear();
        self.status = match self.settings.grid_size {
            Some(n) => format!("Grid: {n} map units"),
            None => "Grid: auto (follows zoom)".into(),
        };
    }

    fn selected_sectors(&self) -> Vec<SectorId> {
        self.selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Sector(s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    /// Wheel decision tree:
    ///   * Sectors mode + non-empty sector selection + Ctrl => brightness
    ///   * Sectors mode + sector selection + Alt           => ceiling height
    ///   * Sectors mode + sector selection + (no mod)      => floor height
    ///   * Shift halves the step magnitude (8 → 1).
    /// Anything else falls back to the existing zoom-about-cursor behavior.
    fn handle_wheel(&mut self, units: f32, pivot: Vec2, viewport: Vec2) {
        let sector_sel: Vec<SectorId> = self
            .selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Sector(s) => Some(*s),
                _ => None,
            })
            .collect();
        let want_sector_op = self.edit_mode == EditMode::Sectors && !sector_sel.is_empty();
        let m = self.modifiers;
        if want_sector_op && (m.control() || m.alt() || m.command() || !m.shift() || m.shift()) {
            // We're going to override zoom only when at least one of
            // Ctrl/Alt/Cmd is held, OR when no modifier is held at all.
            // Plain wheel adjusts floor; with Alt ceiling; with Ctrl brightness.
            // Shift halves the step. A bare wheel without selection / mode
            // still falls through to zoom below.
            let big = if m.shift() { 1 } else { 8 };
            let sign: i32 = if units > 0.0 { 1 } else { -1 };
            let delta = big * sign;
            if m.control() || m.command() {
                self.adjust_sectors_int_field(&sector_sel, SectorIntField::Light, delta);
                return;
            } else if m.alt() {
                self.adjust_sectors_int_field(&sector_sel, SectorIntField::CeilingHeight, delta);
                return;
            } else {
                self.adjust_sectors_int_field(&sector_sel, SectorIntField::FloorHeight, delta);
                return;
            }
        }
        // Default: zoom.
        let factor = (1.15_f32).powf(units);
        self.camera2d.zoom_about(pivot, viewport, factor);
    }

    /// Push a single atomic `Command::Batch` that updates one int field on
    /// each sector in `sectors` by `delta`. Field is clamped per-write.
    fn adjust_sectors_int_field(
        &mut self,
        sectors: &[SectorId],
        field: SectorIntField,
        delta: i32,
    ) {
        let Some(map) = self.map.as_ref() else { return };
        let mut cmds: Vec<Command> = Vec::with_capacity(sectors.len());
        for sid in sectors {
            let Some(sec) = map.sectors.get(*sid) else { continue };
            let old: i32 = match field {
                SectorIntField::FloorHeight => sec.floor_height as i32,
                SectorIntField::CeilingHeight => sec.ceiling_height as i32,
                SectorIntField::Light => sec.light as i32,
                SectorIntField::Tag => sec.tag as i32,
                SectorIntField::Special => sec.special as i32,
            };
            let new = (old + delta).clamp(i16::MIN as i32, i16::MAX as i32);
            if new != old {
                cmds.push(Command::SetSectorIntField {
                    id: *sid,
                    field,
                    old,
                    new,
                });
            }
        }
        self.apply_and_push_batch(cmds);
    }

    fn apply_and_push_batch(&mut self, cmds: Vec<Command>) {
        if cmds.is_empty() {
            return;
        }
        let mut cmd = if cmds.len() == 1 {
            cmds.into_iter().next().unwrap()
        } else {
            Command::Batch(cmds)
        };
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            cmd.apply(map_mut);
            self.undo.push(cmd);
            self.rebuild_geometry_indices();
            self.cache2d.clear();
        }
    }

    /// Apply a linear gradient of an int field across the selected sectors,
    /// from `start` (first selected, by ascending y-then-x of centroid) to
    /// `end` (last). The interpolation is by index, not by spatial distance,
    /// matching GZDB's behavior.
    fn make_sector_gradient(&mut self, field: SectorIntField) {
        let sectors: Vec<SectorId> = self
            .selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Sector(s) => Some(*s),
                _ => None,
            })
            .collect();
        if sectors.len() < 2 {
            self.status = "Gradient: select at least 2 sectors.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else { return };
        let read = |sec: &doombuilder_core::map::MapSector| -> i32 {
            match field {
                SectorIntField::FloorHeight => sec.floor_height as i32,
                SectorIntField::CeilingHeight => sec.ceiling_height as i32,
                SectorIntField::Light => sec.light as i32,
                _ => 0,
            }
        };
        let first = match map.sectors.get(sectors[0]) {
            Some(s) => read(s),
            None => return,
        };
        let last = match map.sectors.get(*sectors.last().unwrap()) {
            Some(s) => read(s),
            None => return,
        };
        let n = sectors.len();
        let mut cmds: Vec<Command> = Vec::with_capacity(n);
        for (i, sid) in sectors.iter().enumerate() {
            let Some(sec) = map.sectors.get(*sid) else { continue };
            let t = i as f32 / (n - 1) as f32;
            let target = (first as f32 + (last as f32 - first as f32) * t).round() as i32;
            let old = read(sec);
            if target != old {
                cmds.push(Command::SetSectorIntField {
                    id: *sid,
                    field,
                    old,
                    new: target.clamp(i16::MIN as i32, i16::MAX as i32),
                });
            }
        }
        let label = match field {
            SectorIntField::Light => "brightness",
            SectorIntField::FloorHeight => "floor",
            SectorIntField::CeilingHeight => "ceiling",
            _ => "field",
        };
        let count = cmds.len();
        self.apply_and_push_batch(cmds);
        self.status = format!("Gradient: {label} across {n} sectors ({count} changed).");
    }

    fn do_join_sectors(&mut self, remove_shared_lines: bool) {
        let sectors: Vec<SectorId> = self
            .selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Sector(s) => Some(*s),
                _ => None,
            })
            .collect();
        if sectors.len() < 2 {
            self.status = "Join: select at least 2 sectors.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else { return };
        match doombuilder_core::edit::compute_join_sectors(map, &sectors, remove_shared_lines) {
            Ok(state) => {
                let absorbed = state.merged_snapshots.len();
                let removed_lines = state.removed_lines.len();
                let mut cmd = Command::JoinSectors(Box::new(state));
                if let Some(map) = self.map.as_mut() {
                    let map_mut = Arc::make_mut(map);
                    cmd.apply(map_mut);
                    self.undo.push(cmd);
                    self.selection = Arc::new(HashSet::new());
                    self.rebuild_geometry_indices();
                    self.cache2d.clear();
                    self.status = if remove_shared_lines {
                        format!(
                            "Merged {absorbed} sector(s), removed {removed_lines} shared linedef(s)."
                        )
                    } else {
                        format!("Joined {absorbed} sector(s).")
                    };
                }
            }
            Err(_) => {
                self.status = "Join: failed (sector missing?).".into();
            }
        }
    }

    /// Make a door from each selected sector: close it (ceiling = floor) and
    /// set perimeter linedef specials to 1 (DR Door). Stored as a batch so a
    /// single undo reverses the whole thing.
    fn do_make_door(&mut self) {
        let sectors: Vec<SectorId> = self
            .selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Sector(s) => Some(*s),
                _ => None,
            })
            .collect();
        if sectors.is_empty() {
            self.status = "Make Door: select one or more sectors.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else { return };
        let mut cmds: Vec<Command> = Vec::new();
        for sid in &sectors {
            let Some(sec) = map.sectors.get(*sid) else { continue };
            let floor = sec.floor_height as i32;
            let ceil = sec.ceiling_height as i32;
            if ceil != floor {
                cmds.push(Command::SetSectorIntField {
                    id: *sid,
                    field: SectorIntField::CeilingHeight,
                    old: ceil,
                    new: floor,
                });
            }
        }
        // Set special=1 on linedefs bordering any selected sector (single
        // door action; not the two-sided "DR Door" trigger, but a simple
        // floor-touches-ceiling closure that engines accept).
        let sec_set: HashSet<SectorId> = sectors.iter().copied().collect();
        for (lid, line) in &map.linedefs {
            let touches = line
                .right
                .and_then(|s| map.sidedefs.get(s).map(|x| x.sector))
                .map(|s| sec_set.contains(&s))
                .unwrap_or(false)
                || line
                    .left
                    .and_then(|s| map.sidedefs.get(s).map(|x| x.sector))
                    .map(|s| sec_set.contains(&s))
                    .unwrap_or(false);
            if touches && line.special == 0 {
                cmds.push(Command::SetLinedefSpecial {
                    id: lid,
                    old: line.special,
                    new: 1,
                });
            }
        }
        if cmds.is_empty() {
            self.status = "Make Door: nothing to change.".into();
            return;
        }
        let count = sectors.len();
        self.apply_and_push_batch(cmds);
        self.status = format!("Made door from {count} sector(s).");
    }

    /// Delete every selected map element (vertices/linedefs/sectors/things)
    /// as a single undoable command. Clears the selection on success.
    fn delete_selection(&mut self) {
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
        if !any {
            return;
        }
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

    /// Round every selected vertex's position to the nearest grid point,
    /// pushed as a single MoveVertices command so undo treats it atomically.
    fn snap_selection_to_grid(&mut self) {
        let Some(map) = self.map.as_ref() else { return };
        let step = self.effective_grid_step().max(1.0);
        let mut moves: Vec<doombuilder_core::edit::VertexMove> = Vec::new();
        // Collect vertex ids from the selection. Linedef selections imply
        // their two endpoints. Sectors are skipped; they aren't a vertex-set.
        let mut targets: HashSet<doombuilder_core::map::VertexId> = HashSet::new();
        for h in self.selection.iter() {
            match h {
                HighlightKind::Vertex(id) => {
                    targets.insert(*id);
                }
                HighlightKind::Linedef(id) => {
                    if let Some(l) = map.linedefs.get(*id) {
                        targets.insert(l.v1);
                        targets.insert(l.v2);
                    }
                }
                _ => {}
            }
        }
        for id in targets {
            if let Some(v) = map.vertices.get(id) {
                let snap_x = ((v.x as f32) / step).round() * step;
                let snap_y = ((v.y as f32) / step).round() * step;
                let dx = (snap_x.round() as i32) - v.x;
                let dy = (snap_y.round() as i32) - v.y;
                if dx != 0 || dy != 0 {
                    moves.push(doombuilder_core::edit::VertexMove { id, dx, dy });
                }
            }
        }
        if moves.is_empty() {
            self.status = "Snap: nothing to snap.".into();
            return;
        }
        let count = moves.len();
        let mut cmd = Command::MoveVertices(moves);
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            cmd.apply(map_mut);
            self.undo.push(cmd);
            self.rebuild_geometry_indices();
            self.cache2d.clear();
            self.status = format!("Snapped {count} vertices to grid.");
        }
    }

    /// Step back one click in the active drawing chain. If the last click
    /// added a new vertex, that vertex and its incoming line are removed; if
    /// it snapped to an existing vertex, only the line is removed.
    /// No-op when no drawing is active.
    fn drawing_remove_last(&mut self) {
        use doombuilder_core::edit::LineEndpoint;
        let Some(drawing) = self.drawing.as_mut() else {
            return;
        };
        if drawing.chain.current_v.is_empty() && drawing.chain.current_l.is_empty() {
            return;
        }
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            // Inspect the most recent linedef-record to decide whether the
            // last click also placed a new vertex.
            let dropped_new_vertex = match drawing.chain.linedefs.last() {
                Some((_, LineEndpoint::New(_), _)) => true,
                Some((_, LineEndpoint::Existing(_), _)) => false,
                // No line yet — only a single starting vertex exists.
                None => true,
            };
            if let Some(lid) = drawing.chain.current_l.pop() {
                map_mut.linedefs.remove(lid);
                drawing.chain.linedefs.pop();
            }
            if dropped_new_vertex {
                if let Some(vid) = drawing.chain.current_v.pop() {
                    map_mut.vertices.remove(vid);
                    drawing.chain.vertex_inserts.pop();
                }
            }
            // Re-establish `last` from the new tail of the chain.
            drawing.last = drawing.chain.linedefs.last().map(|(_, to_ep, line)| {
                let to_vid = match to_ep {
                    LineEndpoint::Existing(v) => *v,
                    LineEndpoint::New(_) => line.v2,
                };
                (to_vid, to_ep.clone())
            });
            // If no lines remain but a starting vertex exists, anchor on it.
            if drawing.last.is_none() {
                if let Some(&vid) = drawing.chain.current_v.last() {
                    let idx = drawing.chain.vertex_inserts.len() - 1;
                    drawing.last = Some((vid, LineEndpoint::New(idx)));
                }
            }
        }
        self.cache2d.clear();
        self.status = "Removed last vertex from drawing.".into();
    }

    /// Snap a drag delta to the current grid step when grid snapping is on.
    /// Returns the integer (dx, dy) to apply to original vertex positions.
    fn snap_drag_delta(&self, start: Vec2, current: Vec2) -> (i32, i32) {
        let raw_dx = current.x - start.x;
        let raw_dy = current.y - start.y;
        if self.settings.snap_to_grid {
            let step = self.effective_grid_step().max(1.0);
            let dx = (raw_dx / step).round() * step;
            let dy = (raw_dy / step).round() * step;
            (dx.round() as i32, dy.round() as i32)
        } else {
            (raw_dx.round() as i32, raw_dy.round() as i32)
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
                let (dx, dy) = self.snap_drag_delta(start, current);
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
                let (dx, dy) = self.snap_drag_delta(start, current);
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
            Some(DragMode::ShapeDraw { .. }) => {
                // Live preview: keep cursor_world in sync with the drag's
                // current position so build_shape_preview() draws against
                // the right anchor + cursor pair.
                self.cursor_world = Some(self.snap_world(current));
                self.cache2d.clear();
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
                let (dx, dy) = self.snap_drag_delta(start, end);
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
                let (dx, dy) = self.snap_drag_delta(start, end);
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
            Some(DragMode::ShapeDraw { origin }) => {
                // Commit the shape: shape_tool_click expects an origin
                // recorded on the tool already (we did that in begin_drag)
                // and treats the second click as the closing corner.
                let end_snapped = self.snap_world(end);
                let _ = origin;
                self.shape_tool_click(end_snapped);
            }
            None => {}
        }
    }

    /// Snap a world position to the active grid step when grid snapping is on.
    fn snap_world(&self, world: Vec2) -> Vec2 {
        if self.settings.snap_to_grid {
            let step = self.effective_grid_step().max(1.0);
            Vec2::new(
                (world.x / step).round() * step,
                (world.y / step).round() * step,
            )
        } else {
            world
        }
    }

    fn begin_drag(&mut self, hit: Option<HighlightKind>, start: Vec2) -> DragMode {
        // Drag-to-draw: if a shape tool is active we always interpret a
        // drag as "draw the shape", ignoring whatever was hit. The drag
        // start becomes the shape's first corner.
        if let Some(d) = self.drawing.as_ref() {
            if matches!(
                d.tool,
                DrawTool::Rectangle { .. }
                    | DrawTool::Ellipse { .. }
                    | DrawTool::Grid { .. }
            ) {
                let snapped = self.snap_world(start);
                // Seed the tool's origin so the live preview tracks the
                // drag start even before the first DragMoved event arrives.
                if let Some(d_mut) = self.drawing.as_mut() {
                    match &mut d_mut.tool {
                        DrawTool::Rectangle { origin, .. }
                        | DrawTool::Ellipse { origin, .. }
                        | DrawTool::Grid { origin, .. } => {
                            *origin = Some(snapped);
                        }
                        _ => {}
                    }
                }
                return DragMode::ShapeDraw { origin: snapped };
            }
        }
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

    fn do_merge_vertices(&mut self) {
        let vertex_ids: Vec<doombuilder_core::map::VertexId> = self
            .selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Vertex(id) => Some(*id),
                _ => None,
            })
            .collect();
        if vertex_ids.len() < 2 {
            self.status = "Merge: select at least two vertices first.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else {
            return;
        };
        match compute_vertex_merge(map, &vertex_ids) {
            Ok(state) => {
                let survivor = state.survivor;
                let mut cmd = Command::MergeVertices(Box::new(state));
                if let Some(map) = self.map.as_mut() {
                    let map_mut = Arc::make_mut(map);
                    cmd.apply(map_mut);
                    self.undo.push(cmd);
                    // Selection collapses to just the survivor.
                    let mut sel = HashSet::new();
                    sel.insert(HighlightKind::Vertex(survivor));
                    self.selection = Arc::new(sel);
                    self.rebuild_geometry_indices();
                    self.cache2d.clear();
                    self.status = format!("Merged {} vertices.", vertex_ids.len());
                }
            }
            Err(doombuilder_core::edit::MergeError::NotEnoughVertices) => {
                self.status = "Merge: select at least two vertices.".into();
            }
            Err(doombuilder_core::edit::MergeError::VertexMissing) => {
                self.status = "Merge: a selected vertex no longer exists.".into();
            }
        }
    }

    fn do_split_lines(&mut self) {
        let line_ids: Vec<doombuilder_core::map::LinedefId> = self
            .selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Linedef(id) => Some(*id),
                _ => None,
            })
            .collect();
        if line_ids.is_empty() {
            self.status = "Split: select one or more linedefs first.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else {
            return;
        };
        match compute_split_lines(map, &line_ids) {
            Ok(state) => {
                let mut cmd = Command::SplitLinedefs(Box::new(state));
                if let Some(map) = self.map.as_mut() {
                    let map_mut = Arc::make_mut(map);
                    cmd.apply(map_mut);
                    self.undo.push(cmd);
                    self.rebuild_geometry_indices();
                    self.cache2d.clear();
                    self.status = format!("Split {} linedef(s).", line_ids.len());
                }
            }
            Err(doombuilder_core::edit::SplitError::NoLines) => {
                self.status = "Split: no linedefs selected.".into();
            }
            Err(doombuilder_core::edit::SplitError::LineMissing) => {
                self.status = "Split: a selected linedef no longer exists.".into();
            }
            Err(doombuilder_core::edit::SplitError::VertexMissing) => {
                self.status = "Split: a selected linedef references a missing vertex.".into();
            }
        }
    }

    /// Save the current map to a temp PWAD and launch the configured engine
    /// with `-iwad <iwad> -file <pwad> -warp ...`. Prompts for engine/IWAD if
    /// either is unset. Spawns detached; engine stdout is not captured.
    fn test_map(&mut self) {
        let Some(map) = self.map.clone() else {
            self.status = "Test: no map to test.".into();
            return;
        };
        let Some(engine) = self.settings.engine_path.clone() else {
            self.status = "Test: set engine path in View > Settings first.".into();
            return;
        };
        let Some(iwad) = self.settings.iwad_path.clone() else {
            self.status = "Test: set IWAD path in View > Settings first.".into();
            return;
        };
        // Stash the test PWAD in the OS temp dir; engines accept absolute paths.
        let pwad_path = std::env::temp_dir().join("doombuilder-test.wad");
        let builder = self.settings.make_node_builder();
        let bytes = match save_map_as_pwad_with(&map, &builder) {
            Ok(b) => b,
            Err(e) => {
                self.status = format!("Test: node builder failed: {e}");
                return;
            }
        };
        if let Err(e) = std::fs::write(&pwad_path, &bytes) {
            self.status = format!("Test: failed to write temp WAD: {e}");
            return;
        }
        let mut cmd = std::process::Command::new(&engine);
        cmd.arg("-iwad").arg(&iwad).arg("-file").arg(&pwad_path);
        if let Some(warp) = warp_args_for(&map.name) {
            for a in warp {
                cmd.arg(a);
            }
        }
        match cmd.spawn() {
            Ok(_) => self.status = format!("Launched engine on {}", map.name),
            Err(e) => self.status = format!("Test: launch failed: {e}"),
        }
    }

    /// Apply sequential tags to the selected sectors starting from the
    /// number in `tag_range_input`. No-op if input doesn't parse or nothing
    /// is selected. Closes the modal on success.
    /// Snapshot the current selection into the clipboard. Pulls in implicit
    /// dependencies (linedef endpoints, sector boundary geometry).
    fn copy_selection(&mut self) {
        let Some(map) = self.map.as_ref() else { return };
        let mut sel_v: HashSet<doombuilder_core::map::VertexId> = HashSet::new();
        let mut sel_l: HashSet<LinedefId> = HashSet::new();
        let mut sel_s: HashSet<SectorId> = HashSet::new();
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
                    sel_s.insert(*s);
                }
                HighlightKind::Thing(t) => {
                    sel_t.insert(*t);
                }
            }
        }
        if sel_v.is_empty() && sel_l.is_empty() && sel_s.is_empty() && sel_t.is_empty() {
            self.status = "Copy: nothing selected.".into();
            return;
        }
        let data = doombuilder_core::edit::build_clipboard(map, &sel_v, &sel_l, &sel_s, &sel_t);
        let summary = format!(
            "{} verts, {} lines, {} sides, {} sectors, {} things",
            data.vertices.len(),
            data.linedefs.len(),
            data.sidedefs.len(),
            data.sectors.len(),
            data.things.len()
        );
        self.clipboard = Some(data);
        self.status = format!("Copied: {summary}.");
    }

    /// Paste the clipboard into the current map. Offsets new geometry by 64
    /// units to the right + down so it visually separates from the original.
    fn paste_selection(&mut self) {
        let Some(data) = self.clipboard.clone() else {
            self.status = "Paste: clipboard is empty.".into();
            return;
        };
        if self.map.is_none() {
            return;
        }
        let state = doombuilder_core::edit::PasteClipboardState {
            data,
            offset: (64, 64),
            ..Default::default()
        };
        let mut cmd = Command::PasteClipboard(Box::new(state));
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            cmd.apply(map_mut);
            // Select the newly-pasted geometry so the user can re-paste,
            // move, or further-modify it immediately.
            if let Command::PasteClipboard(ref s) = cmd {
                let mut sel: HashSet<HighlightKind> = HashSet::new();
                for &id in &s.current_line {
                    sel.insert(HighlightKind::Linedef(id));
                }
                for &id in &s.current_thing {
                    sel.insert(HighlightKind::Thing(id));
                }
                self.selection = Arc::new(sel);
            }
            self.undo.push(cmd);
            self.rebuild_geometry_indices();
            self.cache2d.clear();
            self.status = "Pasted clipboard.".into();
        }
    }

    /// "Paste Properties": apply the clipboard's first element's properties
    /// (non-geometric — textures, flags, heights, light) to every same-kind
    /// element currently selected. Useful for cloning a sector's look or a
    /// thing's flags across many targets.
    fn paste_properties(&mut self) {
        let Some(data) = self.clipboard.as_ref() else {
            self.status = "Paste Properties: clipboard is empty.".into();
            return;
        };
        let mut cmds: Vec<Command> = Vec::new();
        let Some(map) = self.map.as_ref() else { return };
        // Choose source based on what's in the clipboard.
        if let Some(src_sec) = data.sectors.first().cloned() {
            for sid in self.selected_sectors() {
                let Some(s) = map.sectors.get(sid) else { continue };
                cmds.push(Command::SetSectorIntField {
                    id: sid,
                    field: SectorIntField::FloorHeight,
                    old: s.floor_height as i32,
                    new: src_sec.floor_height as i32,
                });
                cmds.push(Command::SetSectorIntField {
                    id: sid,
                    field: SectorIntField::CeilingHeight,
                    old: s.ceiling_height as i32,
                    new: src_sec.ceiling_height as i32,
                });
                cmds.push(Command::SetSectorIntField {
                    id: sid,
                    field: SectorIntField::Light,
                    old: s.light as i32,
                    new: src_sec.light as i32,
                });
                cmds.push(Command::SetSectorIntField {
                    id: sid,
                    field: SectorIntField::Special,
                    old: s.special as i32,
                    new: src_sec.special as i32,
                });
                cmds.push(Command::SetSectorIntField {
                    id: sid,
                    field: SectorIntField::Tag,
                    old: s.tag as i32,
                    new: src_sec.tag as i32,
                });
            }
        }
        if cmds.is_empty() {
            self.status = "Paste Properties: no compatible target selected.".into();
            return;
        }
        let count = cmds.len();
        self.apply_and_push_batch(cmds);
        self.status = format!("Pasted properties onto {count} field(s).");
    }

    /// Apply a discrete transform (rotate 90° CCW, flip H, or flip V) to
    /// every vertex / thing position in the current selection. Pivots around
    /// the selection's centroid so the geometry stays in place visually.
    fn transform_selection(&mut self, kind: SelectionTransform) {
        let Some(map) = self.map.as_ref() else { return };
        // Collect vertex ids: explicit + endpoints of selected lines + sector
        // boundaries.
        let mut vertex_ids: HashSet<doombuilder_core::map::VertexId> = HashSet::new();
        let mut thing_ids: HashSet<ThingId> = HashSet::new();
        for h in self.selection.iter() {
            match h {
                HighlightKind::Vertex(id) => {
                    vertex_ids.insert(*id);
                }
                HighlightKind::Linedef(id) => {
                    if let Some(l) = map.linedefs.get(*id) {
                        vertex_ids.insert(l.v1);
                        vertex_ids.insert(l.v2);
                    }
                }
                HighlightKind::Thing(id) => {
                    thing_ids.insert(*id);
                }
                _ => {}
            }
        }
        if vertex_ids.is_empty() && thing_ids.is_empty() {
            self.status = "Edit Selection: select vertices, linedefs, or things first.".into();
            return;
        }
        // Centroid.
        let mut cx = 0.0_f32;
        let mut cy = 0.0_f32;
        let mut n = 0.0_f32;
        for vid in &vertex_ids {
            if let Some(v) = map.vertices.get(*vid) {
                cx += v.x as f32;
                cy += v.y as f32;
                n += 1.0;
            }
        }
        for tid in &thing_ids {
            if let Some(t) = map.things.get(*tid) {
                cx += t.x as f32;
                cy += t.y as f32;
                n += 1.0;
            }
        }
        if n < 1.0 {
            return;
        }
        cx /= n;
        cy /= n;
        let txform = |x: f32, y: f32| -> (f32, f32) {
            let rx = x - cx;
            let ry = y - cy;
            let (nx, ny) = match kind {
                SelectionTransform::Rotate90 => (-ry, rx), // 90° CCW (math Y-up)
                SelectionTransform::FlipH => (-rx, ry),
                SelectionTransform::FlipV => (rx, -ry),
            };
            (cx + nx, cy + ny)
        };
        // Build a Batch of MoveVertices + MoveThings. Easier than a custom
        // command, and it composes well with undo.
        let mut vmoves: Vec<doombuilder_core::edit::VertexMove> = Vec::new();
        for vid in &vertex_ids {
            let Some(v) = map.vertices.get(*vid) else { continue };
            let (nx, ny) = txform(v.x as f32, v.y as f32);
            let dx = nx.round() as i32 - v.x;
            let dy = ny.round() as i32 - v.y;
            if dx != 0 || dy != 0 {
                vmoves.push(doombuilder_core::edit::VertexMove { id: *vid, dx, dy });
            }
        }
        let mut tmoves: Vec<doombuilder_core::edit::ThingMove> = Vec::new();
        for tid in &thing_ids {
            let Some(t) = map.things.get(*tid) else { continue };
            let (nx, ny) = txform(t.x as f32, t.y as f32);
            let dx = nx.round() as i32 - t.x;
            let dy = ny.round() as i32 - t.y;
            if dx != 0 || dy != 0 {
                tmoves.push(doombuilder_core::edit::ThingMove { id: *tid, dx, dy });
            }
        }
        let mut cmds: Vec<Command> = Vec::new();
        if !vmoves.is_empty() {
            cmds.push(Command::MoveVertices(vmoves));
        }
        if !tmoves.is_empty() {
            cmds.push(Command::MoveThings(tmoves));
        }
        if cmds.is_empty() {
            return;
        }
        self.apply_and_push_batch(cmds);
        self.status = match kind {
            SelectionTransform::Rotate90 => "Rotated selection 90°.".into(),
            SelectionTransform::FlipH => "Flipped selection horizontally.".into(),
            SelectionTransform::FlipV => "Flipped selection vertically.".into(),
        };
    }

    fn apply_tag_range(&mut self) {
        let start = match self.tag_range_input.trim().parse::<i32>() {
            Ok(n) if n >= 0 => n,
            _ => {
                self.status = "Tag Range: enter a non-negative integer.".into();
                return;
            }
        };
        let sectors: Vec<SectorId> = self.selected_sectors();
        if sectors.is_empty() {
            self.status = "Tag Range: select one or more sectors first.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else { return };
        let mut cmds: Vec<Command> = Vec::new();
        for (i, sid) in sectors.iter().enumerate() {
            let Some(s) = map.sectors.get(*sid) else { continue };
            let old = s.tag as i32;
            let new = (start + i as i32).clamp(0, u16::MAX as i32);
            if new != old {
                cmds.push(Command::SetSectorIntField {
                    id: *sid,
                    field: SectorIntField::Tag,
                    old,
                    new,
                });
            }
        }
        let count = cmds.len();
        if count == 0 {
            self.active_picker = None;
            self.status = "Tag Range: nothing changed.".into();
            return;
        }
        self.apply_and_push_batch(cmds);
        self.active_picker = None;
        self.status = format!("Tagged {count} sector(s) starting at {start}.");
    }

    /// Run the map-analysis scanner and return its findings. Cheap; called on
    /// each open of the analysis modal so results stay current.
    fn analyze_map(&self) -> Vec<MapIssue> {
        let Some(map) = self.map.as_ref() else { return Vec::new() };
        let mut issues: Vec<MapIssue> = Vec::new();

        // Lines with no sidedefs on either side — invisible to engines.
        for (lid, l) in &map.linedefs {
            if l.right.is_none() && l.left.is_none() {
                issues.push(MapIssue {
                    severity: IssueSeverity::Warning,
                    category: "Floating line",
                    message: format!("Linedef {lid:?} has no sidedefs (orphan)"),
                });
            }
        }
        // Zero-length linedefs.
        for (lid, l) in &map.linedefs {
            if let (Some(a), Some(b)) =
                (map.vertices.get(l.v1), map.vertices.get(l.v2))
            {
                if a.x == b.x && a.y == b.y {
                    issues.push(MapIssue {
                        severity: IssueSeverity::Error,
                        category: "Zero-length line",
                        message: format!("Linedef {lid:?} has v1 == v2 ({}, {})", a.x, a.y),
                    });
                }
            }
        }
        // Overlapping lines: same (v1, v2) regardless of orientation.
        let mut seen: HashMap<(doombuilder_core::map::VertexId, doombuilder_core::map::VertexId), LinedefId> =
            HashMap::new();
        for (lid, l) in &map.linedefs {
            let key = if l.v1 <= l.v2 { (l.v1, l.v2) } else { (l.v2, l.v1) };
            if let Some(other) = seen.get(&key) {
                issues.push(MapIssue {
                    severity: IssueSeverity::Error,
                    category: "Overlapping lines",
                    message: format!("Linedef {lid:?} overlaps {other:?}"),
                });
            } else {
                seen.insert(key, lid);
            }
        }
        // Dangling vertices — exist but referenced by no linedef.
        let mut used: HashSet<doombuilder_core::map::VertexId> = HashSet::new();
        for (_, l) in &map.linedefs {
            used.insert(l.v1);
            used.insert(l.v2);
        }
        for (vid, v) in &map.vertices {
            if !used.contains(&vid) {
                issues.push(MapIssue {
                    severity: IssueSeverity::Warning,
                    category: "Dangling vertex",
                    message: format!("Vertex {vid:?} at ({}, {}) is unused", v.x, v.y),
                });
            }
        }
        // Sectors with no sidedefs — won't render.
        for (sid, s) in &map.sectors {
            if s.sidedefs.is_empty() {
                issues.push(MapIssue {
                    severity: IssueSeverity::Warning,
                    category: "Empty sector",
                    message: format!("Sector {sid:?} has no sidedefs"),
                });
            }
        }
        // Missing textures (only when a texture set is loaded — otherwise we
        // can't tell what's missing vs unloaded).
        if let Some(textures) = &self.textures {
            let resolve = |name: &doombuilder_core::map::TextureName| -> bool {
                if name.is_empty() {
                    return true; // "-" is fine for missing/no texture
                }
                let s = name.as_str().to_ascii_uppercase();
                textures.textures.contains_key(&s) || textures.flats.contains_key(&s)
            };
            for (sid, s) in &map.sidedefs {
                if !resolve(&s.upper_texture) {
                    issues.push(MapIssue {
                        severity: IssueSeverity::Warning,
                        category: "Missing texture",
                        message: format!(
                            "Sidedef {sid:?} upper = '{}' not in resource WADs",
                            s.upper_texture.as_str()
                        ),
                    });
                }
                if !resolve(&s.lower_texture) {
                    issues.push(MapIssue {
                        severity: IssueSeverity::Warning,
                        category: "Missing texture",
                        message: format!(
                            "Sidedef {sid:?} lower = '{}' not in resource WADs",
                            s.lower_texture.as_str()
                        ),
                    });
                }
                if !resolve(&s.middle_texture) {
                    issues.push(MapIssue {
                        severity: IssueSeverity::Warning,
                        category: "Missing texture",
                        message: format!(
                            "Sidedef {sid:?} middle = '{}' not in resource WADs",
                            s.middle_texture.as_str()
                        ),
                    });
                }
            }
            for (sec_id, s) in &map.sectors {
                if !resolve(&s.floor_texture) {
                    issues.push(MapIssue {
                        severity: IssueSeverity::Warning,
                        category: "Missing flat",
                        message: format!(
                            "Sector {sec_id:?} floor = '{}' not in resource WADs",
                            s.floor_texture.as_str()
                        ),
                    });
                }
                if !resolve(&s.ceiling_texture) {
                    issues.push(MapIssue {
                        severity: IssueSeverity::Warning,
                        category: "Missing flat",
                        message: format!(
                            "Sector {sec_id:?} ceiling = '{}' not in resource WADs",
                            s.ceiling_texture.as_str()
                        ),
                    });
                }
            }
        }
        issues.sort_by(|a, b| {
            (a.severity, a.category).cmp(&(b.severity, b.category))
        });
        issues
    }

    /// Variant of `test_map` that moves the existing Player 1 Start (or
    /// inserts one) to the current cursor position before launching. Useful
    /// to drop into a specific room while iterating. Does not push to undo;
    /// the change is intended to be transient.
    fn test_map_at_cursor(&mut self) {
        let Some(cursor) = self.cursor_world else {
            self.status = "Test at cursor: move cursor over canvas first.".into();
            return;
        };
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            // Find an existing Player 1 Start (kind 1) and move it; insert
            // one if none exists.
            let p1: Option<ThingId> = map_mut
                .things
                .iter()
                .find(|(_, t)| t.kind == 1)
                .map(|(id, _)| id);
            match p1 {
                Some(tid) => {
                    if let Some(t) = map_mut.things.get_mut(tid) {
                        t.x = cursor.x.round() as i32;
                        t.y = cursor.y.round() as i32;
                    }
                }
                None => {
                    map_mut.things.insert(doombuilder_core::map::MapThing {
                        x: cursor.x.round() as i32,
                        y: cursor.y.round() as i32,
                        angle: 0,
                        kind: 1,
                        flags: 7,
                        tid: 0,
                        z: 0,
                        special: 0,
                        args: [0; 5],
                    });
                }
            }
        }
        self.test_map();
    }

    /// For each selected thing, rotate it to face the *normal* of the nearest
    /// linedef, oriented toward the thing's side of the wall (so a monster
    /// placed in front of a wall faces away from it). No-op if no things are
    /// selected or the map has no walls.
    fn do_align_things_to_lines(&mut self) {
        let thing_ids: Vec<ThingId> = self.selected_things();
        if thing_ids.is_empty() {
            self.status = "Align Things: select one or more things first.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else { return };
        let Some(spatial) = self.spatial.as_deref() else {
            self.status = "Align Things: no spatial index available.".into();
            return;
        };
        let mut cmds: Vec<Command> = Vec::new();
        for tid in &thing_ids {
            let Some(t) = map.things.get(*tid) else { continue };
            let tx = t.x as f32;
            let ty = t.y as f32;
            // Map-spanning radius so we always get *some* nearest line.
            let Some(lid) = spatial.nearest_linedef(tx, ty, 65_536.0) else {
                continue;
            };
            let Some(line) = map.linedefs.get(lid) else { continue };
            let (Some(a), Some(b)) =
                (map.vertices.get(line.v1), map.vertices.get(line.v2))
            else {
                continue;
            };
            let dx = (b.x - a.x) as f32;
            let dy = (b.y - a.y) as f32;
            // Right-side normal (math Y-up). Doom angles use the same Y-up
            // convention (0° = East, 90° = North).
            let right_nx = dy;
            let right_ny = -dx;
            // Which side of the line is the thing on?
            let to_thing_x = tx - a.x as f32;
            let to_thing_y = ty - a.y as f32;
            let dot = right_nx * to_thing_x + right_ny * to_thing_y;
            // Flip the normal to point away from the wall toward the thing.
            let (nx, ny) = if dot >= 0.0 {
                (right_nx, right_ny)
            } else {
                (-right_nx, -right_ny)
            };
            let new_angle = doom_angle_of(nx, ny);
            let old = t.angle as i32;
            if new_angle != old {
                cmds.push(Command::SetThingIntField {
                    id: *tid,
                    field: doombuilder_core::edit::ThingIntField::Angle,
                    old,
                    new: new_angle,
                });
            }
        }
        let count = cmds.len();
        if count == 0 {
            self.status = "Align Things: nothing to change.".into();
            return;
        }
        self.apply_and_push_batch(cmds);
        self.status = format!("Aligned {count} thing(s) to nearest linedef.");
    }

    /// Rotate every selected thing so its facing direction points at the
    /// current cursor position. No-op if the cursor isn't inside the
    /// viewport (so we never accidentally face them at the origin).
    fn do_point_things_to_cursor(&mut self) {
        let thing_ids: Vec<ThingId> = self.selected_things();
        if thing_ids.is_empty() {
            self.status = "Point Things: select one or more things first.".into();
            return;
        }
        let Some(cursor) = self.cursor_world else {
            self.status = "Point Things: move cursor over the canvas first.".into();
            return;
        };
        let Some(map) = self.map.as_ref() else { return };
        let mut cmds: Vec<Command> = Vec::new();
        for tid in &thing_ids {
            let Some(t) = map.things.get(*tid) else { continue };
            let dx = cursor.x - t.x as f32;
            let dy = cursor.y - t.y as f32;
            if dx * dx + dy * dy < 1.0 {
                // Cursor on top of the thing — don't introduce a meaningless
                // angle change.
                continue;
            }
            let new_angle = doom_angle_of(dx, dy);
            let old = t.angle as i32;
            if new_angle != old {
                cmds.push(Command::SetThingIntField {
                    id: *tid,
                    field: doombuilder_core::edit::ThingIntField::Angle,
                    old,
                    new: new_angle,
                });
            }
        }
        let count = cmds.len();
        if count == 0 {
            self.status = "Point Things: already pointed at cursor.".into();
            return;
        }
        self.apply_and_push_batch(cmds);
        self.status = format!("Pointed {count} thing(s) at cursor.");
    }

    fn selected_things(&self) -> Vec<ThingId> {
        self.selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Thing(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    fn do_flip_sidedefs(&mut self) {
        let line_ids: Vec<LinedefId> = self.selected_linedefs();
        if line_ids.is_empty() {
            self.status = "Flip Sidedefs: select linedefs first.".into();
            return;
        }
        let count = line_ids.len();
        let mut cmd = Command::FlipSidedefs(line_ids);
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            cmd.apply(map_mut);
            self.undo.push(cmd);
            self.rebuild_geometry_indices();
            self.cache2d.clear();
            self.status = format!("Flipped sidedefs on {count} linedef(s).");
        }
    }

    fn do_stitch_lines(&mut self) {
        let line_ids: Vec<LinedefId> = self.selected_linedefs();
        if line_ids.len() < 2 {
            self.status = "Stitch: select at least two overlapping linedefs.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else { return };
        let merges = doombuilder_core::edit::compute_stitch_lines(map, &line_ids);
        if merges.is_empty() {
            self.status = "Stitch: no overlapping pairs (opposite direction, shared vertices) found.".into();
            return;
        }
        let count = merges.len();
        let mut cmd = Command::StitchLines(merges);
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            cmd.apply(map_mut);
            self.undo.push(cmd);
            // Selection now contains some deleted linedef ids; clear to be safe.
            self.selection = Arc::new(HashSet::new());
            self.rebuild_geometry_indices();
            self.cache2d.clear();
            self.status = format!("Stitched {count} overlapping linedef pair(s).");
        }
    }

    fn do_align_linedefs(&mut self) {
        let line_ids: Vec<LinedefId> = self.selected_linedefs();
        if line_ids.is_empty() {
            self.status = "Align Linedefs: select linedefs first.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else { return };
        let to_flip = doombuilder_core::edit::compute_align_linedefs(map, &line_ids);
        if to_flip.is_empty() {
            self.status = "Align Linedefs: all selected lines already aligned.".into();
            return;
        }
        let count = to_flip.len();
        let mut cmd = Command::FlipLinedefs(to_flip);
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            cmd.apply(map_mut);
            self.undo.push(cmd);
            self.rebuild_geometry_indices();
            self.cache2d.clear();
            self.status = format!("Aligned {count} linedef(s) (front side outward).");
        }
    }

    fn do_auto_align(&mut self, axis: doombuilder_core::edit::AutoAlignAxis) {
        let line_ids: Vec<LinedefId> = self.selected_linedefs();
        if line_ids.is_empty() {
            self.status = "Auto-align: select linedefs first.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else { return };
        let changes = doombuilder_core::edit::compute_auto_align_textures(map, &line_ids, axis);
        if changes.is_empty() {
            self.status = "Auto-align: selection isn't a single chain or has no sidedefs.".into();
            return;
        }
        let count = changes.len();
        let mut cmd = Command::SetSidedefOffsets(changes);
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            cmd.apply(map_mut);
            self.undo.push(cmd);
            self.rebuild_geometry_indices();
            self.cache2d.clear();
            let label = match axis {
                doombuilder_core::edit::AutoAlignAxis::X => "X",
                doombuilder_core::edit::AutoAlignAxis::Y => "Y",
                doombuilder_core::edit::AutoAlignAxis::Both => "X and Y",
            };
            self.status = format!("Auto-aligned {label} on {count} sidedef(s).");
        }
    }

    fn selected_linedefs(&self) -> Vec<LinedefId> {
        self.selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Linedef(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    fn do_flip_lines(&mut self) {
        let line_ids: Vec<doombuilder_core::map::LinedefId> = self
            .selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Linedef(id) => Some(*id),
                _ => None,
            })
            .collect();
        if line_ids.is_empty() {
            self.status = "Flip: select one or more linedefs first.".into();
            return;
        }
        let count = line_ids.len();
        let mut cmd = Command::FlipLinedefs(line_ids);
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            cmd.apply(map_mut);
            self.undo.push(cmd);
            self.rebuild_geometry_indices();
            self.cache2d.clear();
            self.status = format!("Flipped {} linedef(s).", count);
        }
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

    /// Place a default thing at `world` and open the ThingKind picker on
    /// it. Used by the double-click-to-place flow in Things mode.
    fn insert_thing_and_open_picker(&mut self, world: Vec2) {
        // Snapshot grid settings before borrowing `map` mutably.
        let snap_grid_on = self.settings.snap_to_grid;
        let grid_step = self.effective_grid_step().max(1.0);
        let Some(map) = self.map.as_mut() else {
            return;
        };
        let map_mut = Arc::make_mut(map);
        let placed = if snap_grid_on {
            Vec2::new(
                (world.x / grid_step).round() * grid_step,
                (world.y / grid_step).round() * grid_step,
            )
        } else {
            world
        };
        let snapshot = MapThing {
            x: placed.x.round() as i32,
            y: placed.y.round() as i32,
            angle: 0,
            kind: 1, // Player 1 Start: harmless placeholder until the user picks.
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
        // Open the type picker on the freshly-placed thing. Reuses the
        // same panel that fires from "edit thing kind" elsewhere.
        self.active_picker = Some(ActivePicker::ThingKind(id));
        self.picker_filter.clear();
        self.status = "Pick a thing type (Esc to keep placeholder).".into();
    }

    fn drawing_click(&mut self, world: Vec2) {
        // Snap the world position to grid if applicable, before tool dispatch.
        let snapped_world = if self.settings.snap_to_grid {
            let step = self.effective_grid_step().max(1.0);
            Vec2::new(
                (world.x / step).round() * step,
                (world.y / step).round() * step,
            )
        } else {
            world
        };
        // Shape tools have their own click logic.
        let is_shape = matches!(
            self.drawing.as_ref().map(|d| &d.tool),
            Some(DrawTool::Rectangle { .. })
                | Some(DrawTool::Ellipse { .. })
                | Some(DrawTool::Curve { .. })
                | Some(DrawTool::Grid { .. })
        );
        if is_shape {
            self.shape_tool_click(snapped_world);
            return;
        }
        self.drawing_click_free(world);
    }

    fn drawing_click_free(&mut self, world: Vec2) {
        // Snapshot non-mutable values before borrowing `self.drawing`/`self.map`.
        let snap_grid_on = self.settings.snap_to_grid;
        let grid_step = self.effective_grid_step().max(1.0);
        let Some(drawing) = self.drawing.as_mut() else {
            return;
        };
        let Some(map) = self.map.as_mut() else {
            return;
        };
        let map_mut = Arc::make_mut(map);

        // Snap to nearest existing vertex within ~8 px (world units / zoom).
        // The spatial index reflects state before drawing began, so it never
        // includes the in-progress chain's own new vertices. We scan those
        // separately so the user can close a loop onto a vertex they just
        // placed (without this, "close the loop" inserts a duplicate vertex
        // and the chain ends up open).
        let snap_world = (8.0_f32 / self.camera2d.zoom.max(1e-6)).max(2.0);
        let snap_sq = snap_world * snap_world;
        let mut snapped = self
            .spatial
            .as_ref()
            .and_then(|sp| sp.nearest_vertex(world.x, world.y, snap_world));
        if snapped.is_none() {
            let mut best: Option<(f32, doombuilder_core::map::VertexId)> = None;
            for vid in &drawing.chain.current_v {
                if let Some(v) = map_mut.vertices.get(*vid) {
                    let dx = v.x as f32 - world.x;
                    let dy = v.y as f32 - world.y;
                    let d2 = dx * dx + dy * dy;
                    if d2 <= snap_sq && best.map(|(bd, _)| d2 < bd).unwrap_or(true) {
                        best = Some((d2, *vid));
                    }
                }
            }
            snapped = best.map(|(_, id)| id);
        }

        let (target_vid, target_endpoint) = match snapped {
            Some(vid) => (vid, LineEndpoint::Existing(vid)),
            None => {
                let placed = if snap_grid_on {
                    Vec2::new(
                        (world.x / grid_step).round() * grid_step,
                        (world.y / grid_step).round() * grid_step,
                    )
                } else {
                    world
                };
                let vsnap = MapVertex {
                    x: placed.x.round() as i32,
                    y: placed.y.round() as i32,
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
                    fields: Default::default(),
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
        // Auto-commit when this click closes the loop (target == chain's
        // first vertex AND we have at least 3 segments). Two segments would
        // make a degenerate "fold" rather than a polygon, so we wait.
        let closes_loop = drawing.chain.linedefs.len() >= 3
            && drawing
                .chain
                .linedefs
                .first()
                .map(|(_, _, l)| l.v1 == target_vid)
                .unwrap_or(false);
        self.status = format!(
            "Drawing: {} verts, {} lines (Esc cancels, D commits)",
            drawing.chain.current_v.len(),
            drawing.chain.current_l.len()
        );
        if closes_loop {
            // commit_drawing takes self.drawing, applies the chain command,
            // and runs the auto-make-sector path. Drop the &mut borrow first.
            self.commit_drawing();
        }
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
        // If the chain forms a closed, side-less loop, promote it straight
        // into a sector. Soft failure: any error from compute_make_sector
        // (open chain, mixed with existing sided lines, etc.) just leaves
        // the lines selected so the user can run Make Sector manually.
        // Pushed as a separate undo entry so undo peels the sector first
        // and the bare chain second.
        let auto_made_sector = self.try_auto_make_sector(&new_lines);

        let mut sel = HashSet::new();
        for id in &new_lines {
            sel.insert(HighlightKind::Linedef(*id));
        }
        self.selection = Arc::new(sel);
        self.edit_mode = EditMode::Linedefs;
        self.cache2d.clear();
        self.status = if auto_made_sector {
            format!(
                "Drew {} vertices and {} linedefs; closed loop became a sector.",
                count_v, count_l
            )
        } else {
            format!(
                "Drew {} vertices and {} linedefs (selected for Make Sector).",
                count_v, count_l
            )
        };
    }

    /// Try to convert `new_lines` into a sector. Returns true if a sector
    /// was created (and pushed as its own undo entry); false on any failure
    /// from `compute_make_sector` — those failures are silent because they
    /// just mean the user drew an open chain or one that overlaps existing
    /// geometry and we should leave them in control.
    fn try_auto_make_sector(&mut self, new_lines: &[doombuilder_core::map::LinedefId]) -> bool {
        if new_lines.is_empty() {
            return false;
        }
        let Some(map) = self.map.as_ref() else {
            return false;
        };
        let Ok(state) = compute_make_sector(map, new_lines) else {
            return false;
        };
        let mut cmd = Command::MakeSector(Box::new(state));
        let Some(map) = self.map.as_mut() else {
            return false;
        };
        let map_mut = Arc::make_mut(map);
        cmd.apply(map_mut);
        self.undo.push(cmd);
        self.rebuild_geometry_indices();
        true
    }

    /// Build the shape preview snapshot fed to View2D each frame.
    fn build_shape_preview(&self) -> Option<view2d::ShapePreview> {
        let drawing = self.drawing.as_ref()?;
        let cursor = self.cursor_world?;
        match &drawing.tool {
            DrawTool::Rectangle { origin, bevel } => {
                origin.map(|o| view2d::ShapePreview::Rectangle {
                    origin: o,
                    cursor,
                    bevel: *bevel,
                })
            }
            DrawTool::Ellipse { origin, subdivisions } => {
                origin.map(|o| view2d::ShapePreview::Ellipse {
                    origin: o,
                    cursor,
                    subdivisions: *subdivisions,
                })
            }
            DrawTool::Curve { points, subdivisions } => {
                if points.is_empty() {
                    None
                } else {
                    Some(view2d::ShapePreview::Curve {
                        points: points.clone(),
                        cursor,
                        subdivisions: *subdivisions,
                    })
                }
            }
            DrawTool::Grid { origin, cols, rows } => {
                origin.map(|o| view2d::ShapePreview::Grid {
                    origin: o,
                    cursor,
                    cols: *cols,
                    rows: *rows,
                })
            }
            DrawTool::Free => {
                // Rubber-band from the last placed vertex to the cursor.
                // Highlights green when the cursor is within snap range of
                // the chain's first vertex AND we already have ≥ 3 segments
                // (so clicking would close a real polygon, not fold a vee).
                let (last_vid, _) = drawing.last.clone()?;
                let map = self.map.as_ref()?;
                let from_v = map.vertices.get(last_vid)?;
                let from = Vec2::new(from_v.x as f32, from_v.y as f32);
                let snap_world = (8.0_f32 / self.camera2d.zoom.max(1e-6)).max(2.0);
                let snap_sq = snap_world * snap_world;
                let start_v = drawing
                    .chain
                    .linedefs
                    .first()
                    .and_then(|(_, _, l)| map.vertices.get(l.v1));
                let closes_loop = drawing.chain.linedefs.len() >= 3
                    && start_v
                        .map(|sv| {
                            let dx = sv.x as f32 - cursor.x;
                            let dy = sv.y as f32 - cursor.y;
                            dx * dx + dy * dy <= snap_sq
                        })
                        .unwrap_or(false);
                // Compute the length the committed line would actually have:
                // snap to the start vertex when closing, else grid-snap the
                // cursor when grid-snap is on, else use raw cursor coords.
                // Result is in integer map units to match how the engine
                // measures lines.
                let target = if closes_loop {
                    start_v.map(|v| Vec2::new(v.x as f32, v.y as f32)).unwrap_or(cursor)
                } else if self.settings.snap_to_grid {
                    let step = self.effective_grid_step().max(1.0);
                    Vec2::new(
                        (cursor.x / step).round() * step,
                        (cursor.y / step).round() * step,
                    )
                } else {
                    cursor
                };
                let dx = target.x - from.x;
                let dy = target.y - from.y;
                let length = (dx * dx + dy * dy).sqrt().round() as i32;
                Some(view2d::ShapePreview::FreeChain {
                    from,
                    cursor,
                    closes_loop,
                    length,
                })
            }
        }
    }

    /// Activate a non-free drawing tool. Cancels any in-progress free draw,
    /// then opens a fresh DrawingState with the requested tool. Shows a
    /// status hint describing the two- or three-click flow.
    fn start_shape_draw(&mut self, tool: DrawTool) {
        if self.map.is_none() {
            self.status = "Open or create a map first.".into();
            return;
        }
        // If a free-draw chain is open, cancel it so we don't mix tool modes.
        if self.drawing.is_some() {
            self.cancel_drawing();
        }
        let label = tool.label();
        self.drawing = Some(DrawingState {
            tool,
            ..DrawingState::default()
        });
        self.status = match &self.drawing.as_ref().unwrap().tool {
            DrawTool::Rectangle { .. } => {
                "Rectangle: click corner, then opposite corner. +/- for bevel. Esc to cancel.".into()
            }
            DrawTool::Ellipse { .. } => {
                "Ellipse: click corner, then opposite corner. +/- for subdivisions. Esc to cancel.".into()
            }
            DrawTool::Curve { .. } => {
                "Curve: click start, end, then a control point. +/- for subdivisions. Esc to cancel.".into()
            }
            DrawTool::Grid { .. } => {
                "Grid: click corner, then opposite corner. +/- for cell count. Esc to cancel.".into()
            }
            DrawTool::Free => format!("{label} active."),
        };
        self.cache2d.clear();
    }

    /// Tool-aware click handler. First click on a shape tool records the
    /// origin / control points; the final click commits the generated
    /// geometry and clears the drawing state.
    fn shape_tool_click(&mut self, world: Vec2) {
        let Some(drawing) = self.drawing.as_mut() else { return };
        match &mut drawing.tool {
            DrawTool::Rectangle { origin, bevel } => {
                if origin.is_none() {
                    *origin = Some(world);
                    self.status = "Rectangle: click opposite corner.".into();
                    self.cache2d.clear();
                } else {
                    let o = origin.unwrap();
                    let b = *bevel;
                    self.commit_shape(rectangle_vertices(o, world, b), true);
                }
            }
            DrawTool::Ellipse { origin, subdivisions } => {
                if origin.is_none() {
                    *origin = Some(world);
                    self.status = "Ellipse: click opposite corner.".into();
                    self.cache2d.clear();
                } else {
                    let o = origin.unwrap();
                    let n = *subdivisions;
                    self.commit_shape(ellipse_vertices(o, world, n), true);
                }
            }
            DrawTool::Curve { points, subdivisions } => {
                points.push(world);
                if points.len() < 3 {
                    self.status = match points.len() {
                        1 => "Curve: click end point.".into(),
                        2 => "Curve: click control point.".into(),
                        _ => "Curve: ...".into(),
                    };
                    self.cache2d.clear();
                } else {
                    let pts = points.clone();
                    let n = *subdivisions;
                    self.commit_shape(quadratic_bezier_vertices(pts[0], pts[2], pts[1], n), false);
                }
            }
            DrawTool::Grid { origin, cols, rows } => {
                if origin.is_none() {
                    *origin = Some(world);
                    self.status = "Grid: click opposite corner.".into();
                    self.cache2d.clear();
                } else {
                    let o = origin.unwrap();
                    let c = *cols;
                    let r = *rows;
                    self.commit_shape_grid(o, world, c, r);
                }
            }
            DrawTool::Free => {}
        }
    }

    /// Build a `Command::CreateLinedefChain` from a vertex polyline. If
    /// `closed`, the last vertex is connected back to the first.
    fn commit_shape(&mut self, points: Vec<Vec2>, closed: bool) {
        if points.len() < 2 {
            self.cancel_drawing();
            return;
        }
        let chain = chain_from_polyline(&points, closed);
        self.apply_chain_and_commit(chain);
    }

    fn commit_shape_grid(&mut self, a: Vec2, b: Vec2, cols: u32, rows: u32) {
        let chain = chain_from_grid(a, b, cols.max(1), rows.max(1));
        self.apply_chain_and_commit(chain);
    }

    /// Apply a freshly-built chain via `Command::CreateLinedefChain` and
    /// share the "auto-select new lines, switch to Linedefs mode" flourish
    /// with the free-draw commit path.
    fn apply_chain_and_commit(&mut self, mut chain: LinedefChain) {
        if chain.vertex_inserts.is_empty() {
            self.cancel_drawing();
            return;
        }
        // Apply directly to the map (CreateLinedefChain's apply re-inserts).
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            // Insert vertices first, recording their fresh ids.
            chain.current_v.clear();
            for v in &chain.vertex_inserts {
                let id = map_mut.vertices.insert(*v);
                chain.current_v.push(id);
            }
            // Resolve endpoints and insert linedefs.
            chain.current_l.clear();
            let endpoint = |ep: &LineEndpoint, current_v: &[VertexId]| -> Option<VertexId> {
                match ep {
                    LineEndpoint::Existing(v) => Some(*v),
                    LineEndpoint::New(i) => current_v.get(*i).copied(),
                }
            };
            let line_specs: Vec<_> = chain
                .linedefs
                .iter()
                .map(|(from, to, template)| (from.clone(), to.clone(), template.clone()))
                .collect();
            for (from, to, mut template) in line_specs {
                let (Some(v1), Some(v2)) =
                    (endpoint(&from, &chain.current_v), endpoint(&to, &chain.current_v))
                else {
                    continue;
                };
                template.v1 = v1;
                template.v2 = v2;
                let id = map_mut.linedefs.insert(template);
                chain.current_l.push(id);
            }
        }
        let new_lines: Vec<LinedefId> = chain.current_l.clone();
        let count_l = new_lines.len();
        let count_v = chain.current_v.len();
        self.undo.push(Command::CreateLinedefChain(Box::new(chain)));
        // Reset the active shape tool's origin/points so the next drag
        // starts a fresh shape — keeps the user in "Rectangle mode" (or
        // whichever) until they press Esc, matching DCC conventions.
        // Free/Curve tools fall back to nuking the drawing state so they
        // re-enter their own discrete-click flows.
        let keep_tool = match self.drawing.as_ref().map(|d| &d.tool) {
            Some(DrawTool::Rectangle { .. })
            | Some(DrawTool::Ellipse { .. })
            | Some(DrawTool::Grid { .. }) => true,
            _ => false,
        };
        if keep_tool {
            if let Some(d) = self.drawing.as_mut() {
                d.chain = LinedefChain::default();
                d.last = None;
                match &mut d.tool {
                    DrawTool::Rectangle { origin, .. }
                    | DrawTool::Ellipse { origin, .. }
                    | DrawTool::Grid { origin, .. } => {
                        *origin = None;
                    }
                    _ => {}
                }
            }
        } else {
            self.drawing = None;
        }
        self.rebuild_geometry_indices();
        // Auto-select for fluent Make-Sector workflow.
        let mut sel = HashSet::new();
        for id in &new_lines {
            sel.insert(HighlightKind::Linedef(*id));
        }
        self.selection = Arc::new(sel);
        self.edit_mode = EditMode::Linedefs;
        self.cache2d.clear();
        self.status = format!("Shape: {count_v} vertices, {count_l} linedefs.");
    }

    fn adjust_draw_param(&mut self, delta: i32) {
        let Some(drawing) = self.drawing.as_mut() else { return };
        let nudge_u32 = |v: u32, d: i32, min: u32, max: u32| -> u32 {
            ((v as i32 + d).clamp(min as i32, max as i32)) as u32
        };
        match &mut drawing.tool {
            DrawTool::Rectangle { bevel, .. } => {
                *bevel = nudge_u32(*bevel, delta, 0, 64);
                self.status = format!("Rectangle bevel: {}", *bevel);
                self.cache2d.clear();
            }
            DrawTool::Ellipse { subdivisions, .. } => {
                *subdivisions = nudge_u32(*subdivisions, delta, 4, 128);
                self.status = format!("Ellipse subdivisions: {}", *subdivisions);
                self.cache2d.clear();
            }
            DrawTool::Curve { subdivisions, .. } => {
                *subdivisions = nudge_u32(*subdivisions, delta, 2, 128);
                self.status = format!("Curve subdivisions: {}", *subdivisions);
                self.cache2d.clear();
            }
            DrawTool::Grid { cols, rows, .. } => {
                // Single nudge bumps both axes together.
                *cols = nudge_u32(*cols, delta, 1, 64);
                *rows = nudge_u32(*rows, delta, 1, 64);
                self.status = format!("Grid cells: {} x {}", *cols, *rows);
                self.cache2d.clear();
            }
            DrawTool::Free => {}
        }
    }

    /// Convert a selection of linedefs that form a contiguous open chain
    /// into a smoothed bezier curve. Internal vertices are repositioned to
    /// lie on a quadratic bezier defined by the chain's endpoints and a
    /// control point perpendicular to their midline.
    fn curve_selected_lines(&mut self) {
        let line_ids: Vec<LinedefId> = self
            .selection
            .iter()
            .filter_map(|h| match h {
                HighlightKind::Linedef(id) => Some(*id),
                _ => None,
            })
            .collect();
        if line_ids.len() < 2 {
            self.status = "Curve: select at least 2 linedefs.".into();
            return;
        }
        let Some(map) = self.map.as_ref() else { return };
        // Order the lines into a path: walk vertex adjacency. Bail out if
        // the selection isn't a simple chain.
        let mut path = match order_line_chain(map, &line_ids) {
            Some(p) => p,
            None => {
                self.status = "Curve: selected lines do not form a single open chain.".into();
                return;
            }
        };
        if path.len() < 3 {
            self.status = "Curve: need at least 3 vertices.".into();
            return;
        }
        // Control point = midpoint of chord + perpendicular offset half the
        // chord length (so the curve clearly bows away from the chord).
        let start = match map.vertices.get(path[0]) {
            Some(v) => Vec2::new(v.x as f32, v.y as f32),
            None => return,
        };
        let end_idx = *path.last().unwrap();
        let end = match map.vertices.get(end_idx) {
            Some(v) => Vec2::new(v.x as f32, v.y as f32),
            None => return,
        };
        let mid = (start + end) * 0.5;
        let dir = end - start;
        let perp = Vec2::new(-dir.y, dir.x);
        let plen = (perp.x * perp.x + perp.y * perp.y).sqrt().max(1e-3);
        let cp = mid + perp * (0.5 / plen) * ((dir.x * dir.x + dir.y * dir.y).sqrt());
        // Reposition the n-2 interior vertices uniformly along the bezier.
        let n = path.len();
        let mut moves: Vec<doombuilder_core::edit::VertexMove> = Vec::new();
        for i in 1..(n - 1) {
            let t = i as f32 / (n - 1) as f32;
            let p = quadratic_bezier_point(start, cp, end, t);
            let vid = path[i];
            if let Some(v) = map.vertices.get(vid) {
                let dx = (p.x.round() as i32) - v.x;
                let dy = (p.y.round() as i32) - v.y;
                if dx != 0 || dy != 0 {
                    moves.push(doombuilder_core::edit::VertexMove { id: vid, dx, dy });
                }
            }
        }
        path.clear();
        if moves.is_empty() {
            self.status = "Curve: nothing to move.".into();
            return;
        }
        let count = moves.len();
        let mut cmd = Command::MoveVertices(moves);
        if let Some(map) = self.map.as_mut() {
            let map_mut = Arc::make_mut(map);
            cmd.apply(map_mut);
            self.undo.push(cmd);
            self.rebuild_geometry_indices();
            self.cache2d.clear();
            self.status = format!("Curved {count} vertex(es).");
        }
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
        // Keep the status-bar counters honest: any geometry edit can change
        // these, and they were previously only refreshed on map load.
        if let Some(stats) = self.map_stats.as_mut() {
            stats.vertices = map.vertices.len();
            stats.linedefs = map.linedefs.len();
            stats.sidedefs = map.sidedefs.len();
            stats.sectors = map.sectors.len();
            stats.things = map.things.len();
        }
        self.rebuild_sector_fills();
        self.rebuild_geometry3d();
    }

    fn rebuild_sector_fills(&mut self) {
        let Some(map) = &self.map else {
            self.sector_fills = Arc::new(Vec::new());
            return;
        };
        // Wireframe view emits no fills at all.
        if self.settings.view_mode == View2DMode::Wireframe {
            self.sector_fills = Arc::new(Vec::new());
            return;
        }
        let mut tiles: Vec<FillTile> = Vec::new();
        for (sid, mesh) in self.sector_meshes.iter() {
            let fill = match self.settings.view_mode {
                View2DMode::Floor | View2DMode::Ceiling => {
                    let Some(textures) = &self.textures else { continue };
                    let slot = if self.settings.view_mode == View2DMode::Ceiling {
                        doombuilder_render::FillSlot::Ceiling
                    } else {
                        doombuilder_render::FillSlot::Floor
                    };
                    doombuilder_render::rasterise_sector_fill_slot(
                        map, *sid, mesh, textures, slot,
                    )
                }
                View2DMode::Brightness => {
                    let Some(sec) = map.sectors.get(*sid) else { continue };
                    let color = brightness_color(sec.light);
                    doombuilder_render::rasterise_sector_solid(*sid, mesh, color)
                }
                View2DMode::Wireframe => None,
            };
            let Some(fill) = fill else { continue };
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

    /// In vertex mode: if `world` is within hit-radius of a linedef, split
    /// that linedef at the click position and return true. Returns false
    /// when no linedef is near.
    fn try_insert_vertex_on_line(&mut self, world: Vec2) -> bool {
        let Some(spatial) = self.spatial.as_ref() else {
            return false;
        };
        let zoom = self.camera2d.zoom.max(1e-6);
        let radius = 8.0 / zoom;
        let Some(line_id) = spatial.nearest_linedef(world.x, world.y, radius) else {
            return false;
        };
        let Some(map) = self.map.as_ref() else {
            return false;
        };
        let snapped = if self.settings.snap_to_grid {
            let step = self.effective_grid_step().max(1.0);
            Vec2::new(
                (world.x / step).round() * step,
                (world.y / step).round() * step,
            )
        } else {
            world
        };
        match compute_insert_vertex_on_line(map, line_id, snapped.x, snapped.y) {
            Ok(state) => {
                let mut cmd = Command::SplitLinedefs(Box::new(state));
                if let Some(map) = self.map.as_mut() {
                    let map_mut = Arc::make_mut(map);
                    cmd.apply(map_mut);
                    self.undo.push(cmd);
                    self.rebuild_geometry_indices();
                    self.cache2d.clear();
                    self.status = "Inserted vertex on linedef.".into();
                }
                true
            }
            Err(_) => false,
        }
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
        // When the 3D preview panel is enabled, give it a fixed slot in the
        // top-right of the viewport row. The preview pad fills the rest of
        // that right column with transparent space so the viewport keeps the
        // remaining width and full height beside it.
        let middle: Element<'_, Message> = if self.settings.show_3d_overlay
            && self.map.is_some()
            && self.mode == Mode::View2D
        {
            row![viewport, self.view3d_side_panel()]
                .spacing(0)
                .height(Length::Fill)
                .into()
        } else {
            viewport
        };
        let mut layout = column![menu, toolbar, middle].spacing(0);
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
        let mut bar = row![
            menu_picker("File", FILE_MENU_ITEMS, dispatch_file),
            menu_picker("Edit", EDIT_MENU_ITEMS, dispatch_edit),
            menu_picker("Mode", MODE_MENU_ITEMS, dispatch_mode),
            menu_picker("View", VIEW_MENU_ITEMS, dispatch_view),
            menu_picker("Tools", TOOLS_MENU_ITEMS, dispatch_tools),
            menu_picker("Help", HELP_MENU_ITEMS, dispatch_help),
        ]
        .spacing(2)
        .padding(2)
        .align_y(iced::Alignment::Center);
        if !self.settings.recent_files.is_empty() {
            bar = bar.push(self.recent_files_picker());
        }
        container(bar)
            .style(menu_bar_style)
            .width(Length::Fill)
            .into()
    }

    fn recent_files_picker(&self) -> Element<'_, Message> {
        // Use a struct that carries the index so dispatch is O(1) and unique
        // even when two paths have the same filename.
        #[derive(Debug, Clone, PartialEq)]
        struct RecentEntry {
            idx: usize,
            label: String,
        }
        impl std::fmt::Display for RecentEntry {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.label)
            }
        }
        let entries: Vec<RecentEntry> = self
            .settings
            .recent_files
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string());
                RecentEntry { idx, label: name }
            })
            .collect();
        pick_list(entries, None::<RecentEntry>, |e| Message::OpenRecent(e.idx))
            .placeholder("Recent")
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
            icons::icon_cmd_btn(icons::NEW_DOC, "New map (Doom format)", Message::NewMap(MapFormat::Doom)),
            icons::icon_cmd_btn(icons::FOLDER_OPEN, "Open WAD\u{2026}", Message::OpenWadRequested),
            icons::icon_cmd_btn(icons::LOAD_RESOURCES, "Load resource WAD (textures + sprites only)\u{2026}", Message::LoadResourcesRequested),
            icons::icon_cmd_btn(icons::SAVE_DISK, "Save Map As\u{2026}", Message::SaveMapRequested),
            icons::icon_cmd_btn(icons::PLAY_TRIANGLE, "Test map in engine (F5)", Message::TestMap),
            vertical_separator(),
            icons::icon_cmd_btn(icons::UNDO, "Undo (\u{2318}Z)", Message::Undo),
            icons::icon_cmd_btn(icons::REDO, "Redo (\u{2318}\u{21E7}Z)", Message::Redo),
            vertical_separator(),
            map_picker,
            vertical_separator(),
            icons::icon_btn(icons::VIEW_2D, "2D View", Message::Mode(Mode::View2D), self.mode == Mode::View2D),
            icons::icon_btn(icons::VIEW_3D, "3D View", Message::Mode(Mode::View3D), self.mode == Mode::View3D),
            vertical_separator(),
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
            icons::icon_cmd_btn(icons::SPLIT_LINE, "Split selected linedefs at midpoint", Message::SplitLines),
            icons::icon_cmd_btn(icons::MERGE_VERTS, "Merge selected vertices", Message::MergeVertices),
            icons::icon_cmd_btn(icons::FLIP_LINE, "Flip selected linedefs (swap front/back)", Message::FlipLines),
            icons::icon_btn(icons::TEXTURES, "Show sector textures", Message::ToggleTextures, self.settings.show_textures),
            icons::icon_cmd_btn(icons::SETTINGS_GEAR, "Settings\u{2026}", Message::OpenSettings),
        ]
        .spacing(2)
        .padding(4)
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
                    fills: match self.settings.view_mode {
                        // Wireframe never shows fills.
                        View2DMode::Wireframe => Arc::new(Vec::new()),
                        // For Floor/Ceiling modes, `show_textures` still acts
                        // as a quick "strip texture overlay" toggle so the
                        // existing toolbar button keeps working.
                        View2DMode::Floor | View2DMode::Ceiling => {
                            if self.settings.show_textures {
                                self.sector_fills.clone()
                            } else {
                                Arc::new(Vec::new())
                            }
                        }
                        // Brightness is the entire point of the view — always
                        // show the fills regardless of the textures toggle.
                        View2DMode::Brightness => self.sector_fills.clone(),
                    },
                    config: self.config.clone(),
                    edit_mode: self.edit_mode,
                    sprite_handles: self.sprite_handles.clone(),
                    sprite_dims: self.sprite_dims.clone(),
                    settings: self.settings.clone(),
                    pan_override: self.space_held,
                    shape_preview: self.build_shape_preview(),
                    opaque_fills: self.settings.view_mode == View2DMode::Brightness,
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
        let grid = self.effective_grid_step();
        let grid_label = if self.settings.grid_size.is_some() {
            format!("{grid:.0}")
        } else {
            format!("{grid:.0} (auto)")
        };
        let zoom = self.camera2d.zoom;
        let right = if self.map.is_some() {
            let sel = self.selection.len();
            let sel_part = if sel > 0 {
                format!("Sel: {sel}   ")
            } else {
                String::new()
            };
            format!("{sel_part}Grid: {grid_label}   Zoom: {zoom:.3}")
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
            Some(ActivePicker::GoToCoords) => self.go_to_coords_panel(),
            Some(ActivePicker::MapStats) => self.map_stats_panel(),
            Some(ActivePicker::MapAnalysis) => self.map_analysis_panel(),
            Some(ActivePicker::UsedTags) => self.used_tags_panel(),
            Some(ActivePicker::TagRange) => self.tag_range_panel(),
            Some(ActivePicker::ThingTypes) => self.thing_types_panel(),
            Some(ActivePicker::MapInWad) => self.map_in_wad_panel(),
            Some(ActivePicker::MapOptions) => self.map_options_panel(),
            Some(ActivePicker::About) => self.about_panel(),
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

    fn go_to_coords_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Go To Coordinates").size(18),
            Space::new().width(Length::Fill),
            button("Close")
                .style(style::win32_standard_button)
                .on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let x_input = row![
            text("X: ").size(13).width(Length::Fixed(24.0)),
            text_input("X", &self.go_to_coords_x)
                .on_input(Message::GoToCoordsXChanged)
                .on_submit(Message::GoToCoordsSubmit)
                .padding(6)
                .style(style::win32_text_input)
                .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let y_input = row![
            text("Y: ").size(13).width(Length::Fixed(24.0)),
            text_input("Y", &self.go_to_coords_y)
                .on_input(Message::GoToCoordsYChanged)
                .on_submit(Message::GoToCoordsSubmit)
                .padding(6)
                .style(style::win32_text_input)
                .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let go = button("Go")
            .style(style::win32_standard_button)
            .on_press(Message::GoToCoordsSubmit);

        column![title_row, x_input, y_input, row![Space::new().width(Length::Fill), go]]
            .spacing(12)
            .padding(16)
            .into()
    }

    fn map_stats_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Map Statistics").size(18),
            Space::new().width(Length::Fill),
            button("Close")
                .style(style::win32_standard_button)
                .on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let Some(map) = self.map.as_ref() else {
            return column![title_row, text("No map loaded.").size(13)]
                .spacing(12)
                .padding(16)
                .into();
        };

        // ---- Header summary (name + format) ----
        let format_str = match map.format {
            MapFormat::Doom => "Doom",
            MapFormat::Hexen => "Hexen",
        };
        let header = row![
            text(format!("{} ({})", map.name, format_str)).size(15),
        ];

        // ---- Geometry counts grid ----
        let counts = [
            ("Vertices", map.vertices.len()),
            ("Linedefs", map.linedefs.len()),
            ("Sidedefs", map.sidedefs.len()),
            ("Sectors", map.sectors.len()),
            ("Things", map.things.len()),
        ];
        let count_rows: Vec<Element<'_, Message>> = counts
            .iter()
            .map(|(label, n)| {
                row![
                    text(*label).size(12).width(Length::Fixed(140.0)),
                    text(n.to_string()).size(12).width(Length::Fill),
                ]
                .into()
            })
            .collect();

        // ---- Map AABB ----
        // Split onto two lines so the long coord string doesn't clip the
        // value column when the modal is the default 760 px wide.
        let (bounds_coords, bounds_size) = match map_aabb(map) {
            Some((min, max)) => (
                format!(
                    "({:.0}, {:.0}) to ({:.0}, {:.0})",
                    min.x, min.y, max.x, max.y
                ),
                format!("{:.0} x {:.0}", max.x - min.x, max.y - min.y),
            ),
            None => ("(empty)".to_string(), String::new()),
        };

        // ---- Linedef breakdown ----
        let mut one_sided = 0usize;
        let mut two_sided = 0usize;
        let mut with_special = 0usize;
        let mut tagged_lines = 0usize;
        for (_, l) in &map.linedefs {
            if l.left.is_some() && l.right.is_some() {
                two_sided += 1;
            } else if l.right.is_some() || l.left.is_some() {
                one_sided += 1;
            }
            if l.special != 0 {
                with_special += 1;
            }
            if l.tag != 0 {
                tagged_lines += 1;
            }
        }

        // ---- Sector breakdown ----
        let mut sec_with_special = 0usize;
        let mut tagged_secs = 0usize;
        let mut light_min: i32 = i32::MAX;
        let mut light_max: i32 = i32::MIN;
        let mut light_sum: i64 = 0;
        let mut floor_min: i32 = i32::MAX;
        let mut floor_max: i32 = i32::MIN;
        let mut ceil_min: i32 = i32::MAX;
        let mut ceil_max: i32 = i32::MIN;
        let mut unique_tags: HashSet<u16> = HashSet::new();
        for (_, s) in &map.sectors {
            if s.special != 0 {
                sec_with_special += 1;
            }
            if s.tag != 0 {
                tagged_secs += 1;
                unique_tags.insert(s.tag);
            }
            let l = s.light as i32;
            light_min = light_min.min(l);
            light_max = light_max.max(l);
            light_sum += l as i64;
            floor_min = floor_min.min(s.floor_height as i32);
            floor_max = floor_max.max(s.floor_height as i32);
            ceil_min = ceil_min.min(s.ceiling_height as i32);
            ceil_max = ceil_max.max(s.ceiling_height as i32);
        }
        let n_sec = map.sectors.len().max(1) as i64;
        let avg_light = if map.sectors.is_empty() { 0 } else { (light_sum / n_sec) as i32 };

        // ---- Thing breakdown by category and skill flag ----
        let mut by_cat: HashMap<String, usize> = HashMap::new();
        let mut skill_easy = 0usize;
        let mut skill_medium = 0usize;
        let mut skill_hard = 0usize;
        let mut multiplayer = 0usize;
        for (_, t) in &map.things {
            let cat = self
                .config
                .thing_type(t.kind)
                .map(|tt| tt.category.clone())
                .unwrap_or_else(|| "(uncategorised)".to_string());
            *by_cat.entry(cat).or_insert(0) += 1;
            if t.flags & 0x01 != 0 {
                skill_easy += 1;
            }
            if t.flags & 0x02 != 0 {
                skill_medium += 1;
            }
            if t.flags & 0x04 != 0 {
                skill_hard += 1;
            }
            if t.flags & 0x10 != 0 {
                multiplayer += 1;
            }
        }
        let mut cat_entries: Vec<(String, usize)> = by_cat.into_iter().collect();
        cat_entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let cat_rows: Vec<Element<'_, Message>> = cat_entries
            .into_iter()
            .map(|(name, n)| {
                row![
                    text(name).size(12).width(Length::Fixed(140.0)),
                    text(n.to_string()).size(12).width(Length::Fill),
                ]
                .into()
            })
            .collect();

        // ---- Compose ----
        let kv = |label: &'static str, val: String| -> Element<'_, Message> {
            row![
                text(label).size(12).width(Length::Fixed(140.0)),
                text(val).size(12).width(Length::Fill),
            ]
            .into()
        };

        let mut geometry_rows: Vec<Element<'_, Message>> = count_rows;
        geometry_rows.push(kv("Bounds", bounds_coords));
        if !bounds_size.is_empty() {
            geometry_rows.push(kv("Size", bounds_size));
        }
        let geometry_section = column![
            text("Geometry").size(14),
            column(geometry_rows).spacing(2),
        ]
        .spacing(6);

        let lines_section = column![
            text("Linedefs").size(14),
            kv("One-sided", one_sided.to_string()),
            kv("Two-sided", two_sided.to_string()),
            kv("With special", with_special.to_string()),
            kv("Tagged", tagged_lines.to_string()),
        ]
        .spacing(2);

        let sectors_section = if map.sectors.is_empty() {
            column![text("Sectors").size(14), text("(none)").size(12)].spacing(2)
        } else {
            column![
                text("Sectors").size(14),
                kv("With special", sec_with_special.to_string()),
                kv("Tagged", format!("{tagged_secs} ({} unique)", unique_tags.len())),
                kv(
                    "Light",
                    format!("min {light_min}  avg {avg_light}  max {light_max}"),
                ),
                kv(
                    "Floor height",
                    format!("min {floor_min}  max {floor_max}"),
                ),
                kv(
                    "Ceiling height",
                    format!("min {ceil_min}  max {ceil_max}"),
                ),
            ]
            .spacing(2)
        };

        let things_section = if map.things.is_empty() {
            column![text("Things").size(14), text("(none)").size(12)].spacing(2)
        } else {
            column![
                text("Things").size(14),
                kv("Easy spawns", skill_easy.to_string()),
                kv("Medium spawns", skill_medium.to_string()),
                kv("Hard spawns", skill_hard.to_string()),
                kv("Multiplayer-only", multiplayer.to_string()),
                Element::from(Space::new().height(Length::Fixed(4.0))),
                Element::from(text("By category").size(13)),
                Element::from(column(cat_rows).spacing(2)),
            ]
            .spacing(2)
        };

        let body = column![
            header,
            geometry_section,
            lines_section,
            sectors_section,
            things_section,
        ]
        .spacing(14)
        .width(Length::Fill);

        column![
            title_row,
            scrollable(body).height(Length::Fill).width(Length::Fill),
        ]
        .spacing(12)
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn map_analysis_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Map Analysis").size(18),
            Space::new().width(Length::Fill),
            button("Close")
                .style(style::win32_standard_button)
                .on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let issues = self.analyze_map();
        let summary = if issues.is_empty() {
            "No issues found.".to_string()
        } else {
            let errors = issues
                .iter()
                .filter(|i| i.severity == IssueSeverity::Error)
                .count();
            let warns = issues.len() - errors;
            format!("{errors} error(s), {warns} warning(s)")
        };
        let mut rows: Vec<Element<'_, Message>> = Vec::with_capacity(issues.len());
        for issue in issues {
            let sev_color = match issue.severity {
                IssueSeverity::Error => iced::Color::from_rgb(1.0, 0.45, 0.45),
                IssueSeverity::Warning => iced::Color::from_rgb(0.95, 0.85, 0.35),
            };
            rows.push(
                row![
                    text(issue.severity.label())
                        .size(11)
                        .width(Length::Fixed(60.0))
                        .color(sev_color),
                    text(issue.category)
                        .size(11)
                        .width(Length::Fixed(160.0)),
                    text(issue.message).size(11).width(Length::Fill),
                ]
                .spacing(8)
                .into(),
            );
        }
        column![
            title_row,
            text(summary).size(13),
            scrollable(column(rows).spacing(2)).height(Length::Fill),
        ]
        .spacing(12)
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn used_tags_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Used Tags").size(18),
            Space::new().width(Length::Fill),
            button("Close")
                .style(style::win32_standard_button)
                .on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let Some(map) = self.map.as_ref() else {
            return column![title_row, text("No map loaded.").size(13)]
                .spacing(12)
                .padding(16)
                .into();
        };
        // tag -> (sectors, lines, things)
        let mut counts: HashMap<u16, [usize; 3]> = HashMap::new();
        for (_, s) in &map.sectors {
            if s.tag != 0 {
                counts.entry(s.tag).or_insert([0; 3])[0] += 1;
            }
        }
        for (_, l) in &map.linedefs {
            if l.tag != 0 {
                counts.entry(l.tag).or_insert([0; 3])[1] += 1;
            }
        }
        for (_, t) in &map.things {
            if t.tid != 0 {
                counts.entry(t.tid).or_insert([0; 3])[2] += 1;
            }
        }
        let mut entries: Vec<(u16, [usize; 3])> = counts.into_iter().collect();
        entries.sort_by_key(|(t, _)| *t);
        let header = row![
            text("Tag").size(12).width(Length::Fixed(60.0)),
            text("Sectors").size(12).width(Length::Fixed(80.0)),
            text("Lines").size(12).width(Length::Fixed(80.0)),
            text("Thing TIDs").size(12).width(Length::Fixed(100.0)),
        ];
        let rows: Vec<Element<'_, Message>> = entries
            .into_iter()
            .map(|(tag, c)| {
                row![
                    text(tag.to_string()).size(11).width(Length::Fixed(60.0)),
                    text(c[0].to_string()).size(11).width(Length::Fixed(80.0)),
                    text(c[1].to_string()).size(11).width(Length::Fixed(80.0)),
                    text(c[2].to_string()).size(11).width(Length::Fixed(100.0)),
                ]
                .into()
            })
            .collect();
        let body: Element<'_, Message> = if rows.is_empty() {
            text("No tags in use.").size(12).into()
        } else {
            column![header, scrollable(column(rows).spacing(2)).height(Length::Fill)]
                .spacing(8)
                .into()
        };
        column![title_row, body]
            .spacing(12)
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn tag_range_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Tag Range").size(18),
            Space::new().width(Length::Fill),
            button("Close")
                .style(style::win32_standard_button)
                .on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let sel_count = self.selected_sectors().len();
        column![
            title_row,
            text(format!(
                "Assigns sequential tags starting from N to the {} selected sector(s) in selection order.",
                sel_count
            ))
            .size(12),
            row![
                text("Start at: ").size(13),
                text_input("1", &self.tag_range_input)
                    .on_input(Message::TagRangeInputChanged)
                    .on_submit(Message::TagRangeApply)
                    .padding(6)
                    .style(style::win32_text_input)
                    .width(Length::Fixed(120.0)),
                Space::new().width(Length::Fill),
                button("Apply")
                    .style(style::win32_standard_button)
                    .on_press(Message::TagRangeApply),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12)
        .padding(16)
        .width(Length::Fill)
        .into()
    }

    fn thing_types_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Thing Types").size(18),
            Space::new().width(Length::Fill),
            button("Close")
                .style(style::win32_standard_button)
                .on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let Some(map) = self.map.as_ref() else {
            return column![title_row, text("No map loaded.").size(13)]
                .spacing(12)
                .padding(16)
                .into();
        };
        // kind -> count
        let mut counts: HashMap<u16, usize> = HashMap::new();
        for (_, t) in &map.things {
            *counts.entry(t.kind).or_insert(0) += 1;
        }
        let mut entries: Vec<(u16, usize)> = counts.into_iter().collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let header = row![
            text("Kind").size(12).width(Length::Fixed(60.0)),
            text("Count").size(12).width(Length::Fixed(60.0)),
            text("Title").size(12).width(Length::Fixed(220.0)),
            text("Category").size(12).width(Length::Fill),
        ];
        let rows: Vec<Element<'_, Message>> = entries
            .into_iter()
            .map(|(kind, n)| {
                let info = self.config.thing_type(kind);
                let title = info
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| "(unknown)".into());
                let cat = info
                    .map(|t| t.category.clone())
                    .unwrap_or_else(|| String::new());
                row![
                    text(kind.to_string()).size(11).width(Length::Fixed(60.0)),
                    text(n.to_string()).size(11).width(Length::Fixed(60.0)),
                    text(title).size(11).width(Length::Fixed(220.0)),
                    text(cat).size(11).width(Length::Fill),
                ]
                .into()
            })
            .collect();
        let body: Element<'_, Message> = if rows.is_empty() {
            text("No things on the map.").size(12).into()
        } else {
            column![header, scrollable(column(rows).spacing(2)).height(Length::Fill)]
                .spacing(8)
                .into()
        };
        column![title_row, body]
            .spacing(12)
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn settings_panel(&self) -> Element<'_, Message> {
        // ── Header bar ────────────────────────────────────────────────
        // Big title + dim subtitle on the left, Close on the right.
        // Sits above the scrolling card stack so it stays put while
        // the content scrolls past.
        let header = container(
            row![
                column![
                    text("Settings").size(22),
                    text("Configure DoomBuilder")
                        .size(12)
                        .color(palette::active().text_dim),
                ]
                .spacing(2),
                Space::new().width(Length::Fill),
                button("Done")
                    .style(style::win32_standard_button)
                    .on_press(Message::ClosePicker),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
        )
        .padding([16, 20])
        .style(settings_header_style)
        .width(Length::Fill);

        // ── Appearance card ───────────────────────────────────────────
        let appearance = settings_card(
            "Appearance",
            "Theme and how the 2D viewport is drawn.",
            column![
                labelled_field(
                    "Theme",
                    pick_list(
                        ThemeKind::all().to_vec(),
                        Some(self.settings.theme),
                        Message::SetTheme,
                    )
                    .placeholder("Theme")
                    .into(),
                ),
                Space::new().height(Length::Fixed(8.0)),
                text("2D viewport display")
                    .size(12)
                    .color(palette::active().text_dim),
                // Two columns of toggles so they don't form a long stripe.
                row![
                    column![
                        toggle_row(self, SettingKey::ShowTextures),
                        toggle_row(self, SettingKey::ShowSprites),
                        toggle_row(self, SettingKey::ShowGrid),
                    ]
                    .spacing(10)
                    .width(Length::FillPortion(1)),
                    column![
                        toggle_row(self, SettingKey::ShowThings),
                        toggle_row(self, SettingKey::AlwaysShowVertices),
                        toggle_row(self, SettingKey::SnapToGrid),
                    ]
                    .spacing(10)
                    .width(Length::FillPortion(1)),
                ]
                .spacing(16),
            ]
            .spacing(8)
            .into(),
        );

        // ── Test Map card ─────────────────────────────────────────────
        let engine_set = self.settings.engine_path.is_some();
        let iwad_set = self.settings.iwad_path.is_some();
        let test_card = settings_card(
            "Test Map",
            "Engine + IWAD used by F5. Both must be set to launch.",
            column![
                path_field(
                    "Engine",
                    self.settings.engine_path.as_ref(),
                    engine_set,
                    Message::PickEngineRequested,
                ),
                path_field(
                    "IWAD",
                    self.settings.iwad_path.as_ref(),
                    iwad_set,
                    Message::PickIwadRequested,
                ),
            ]
            .spacing(10)
            .into(),
        );

        // ── Node Builder card ─────────────────────────────────────────
        let needs_zdbsp = matches!(self.settings.node_builder, NodeBuilderKind::Zdbsp);
        let zdbsp_set = self.settings.zdbsp_path.is_some();
        let nodes_card = settings_card(
            "Node Builder",
            "Which BSP/blockmap/reject builder to run when saving.",
            column![
                labelled_field(
                    "Builder",
                    pick_list(
                        NodeBuilderKind::ALL.to_vec(),
                        Some(self.settings.node_builder),
                        Message::SetNodeBuilder,
                    )
                    .placeholder("Node builder")
                    .into(),
                ),
                // Only show the path row when zdbsp is selected — otherwise
                // it's noise. The pip stays red until a path is set.
                if needs_zdbsp {
                    path_field(
                        "zdbsp",
                        self.settings.zdbsp_path.as_ref(),
                        zdbsp_set,
                        Message::PickZdbspRequested,
                    )
                } else {
                    Space::new().height(Length::Fixed(0.0)).into()
                },
            ]
            .spacing(10)
            .into(),
        );

        let body = scrollable(
            column![appearance, test_card, nodes_card]
                .spacing(14)
                .padding([0, 20])
                .padding([14, 20]),
        )
        .height(Length::Fill);

        column![header, body].spacing(0).into()
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

    /// Modal list of every map in the currently-loaded WAD. Click a row to
    /// load it (same pathway as the toolbar's map picker). Filter narrows
    /// the list by lump-name substring; the currently-loaded map is tagged.
    fn map_in_wad_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Open map in current WAD").size(18),
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
        let filtered: Vec<&String> = self
            .maps
            .iter()
            .filter(|n| q.is_empty() || n.to_ascii_lowercase().contains(&q))
            .collect();
        let count_text = text(format!("{} of {} maps", filtered.len(), self.maps.len())).size(12);

        let mut rows_col: Vec<Element<'_, Message>> = Vec::with_capacity(filtered.len());
        for name in &filtered {
            let is_current = self.selected_map.as_deref() == Some(name.as_str());
            let label = if is_current {
                format!("{}   (loaded)", name)
            } else {
                (*name).clone()
            };
            let row_btn = button(text(label).size(13))
                .padding(8)
                .style(style::win32_toolbar_button)
                .width(Length::Fill)
                .on_press(Message::MapSelected((*name).clone()));
            rows_col.push(row_btn.into());
        }
        let list: Element<'_, Message> = if rows_col.is_empty() {
            container(text("No maps match filter.").size(12))
                .padding(12)
                .into()
        } else {
            scrollable(column(rows_col).spacing(2))
                .height(Length::Fill)
                .into()
        };

        column![title_row, search, count_text, list]
            .spacing(8)
            .padding(16)
            .into()
    }

    /// Help → About modal. Static info about the build.
    fn about_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("About DoomBuilder").size(18),
            Space::new().width(Length::Fill),
            button("Close")
                .style(style::win32_standard_button)
                .on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let version = env!("CARGO_PKG_VERSION");
        let body = column![
            text(format!("DoomBuilder v{version}")).size(15),
            text("Rust Doom map editor").size(12),
            Space::new().height(Length::Fixed(8.0)),
            text(format!("Active config: {}", self.current_config_name)).size(12),
            text(format!(
                "Loaded WAD: {}",
                self.wad_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(none)".into())
            ))
            .size(12),
        ]
        .spacing(4);

        column![title_row, body].spacing(12).padding(16).into()
    }

    /// F2 modal showing the map's metadata. Read-mostly: name is editable
    /// (Doom's 8-char uppercase rules enforced on input), format is shown
    /// as a label since changing it isn't a safe in-place op.
    fn map_options_panel(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Map Options").size(18),
            Space::new().width(Length::Fill),
            button("Close")
                .style(style::win32_standard_button)
                .on_press(Message::ClosePicker),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let format_label = match self.map.as_ref().map(|m| m.format) {
            Some(MapFormat::Doom) => "Doom".to_string(),
            Some(MapFormat::Hexen) => "Hexen".to_string(),
            None => "(no map)".to_string(),
        };

        let name_input = text_input("MAP01", &self.map_name_buffer)
            .on_input(Message::MapNameInputChanged)
            .on_submit(Message::MapNameSubmit)
            .padding(6)
            .style(style::win32_text_input)
            .width(Length::Fixed(220.0));

        let body = column![
            row![
                text("Lump name:").size(13).width(Length::Fixed(110.0)),
                name_input,
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            row![
                text("Format:").size(13).width(Length::Fixed(110.0)),
                text(format_label).size(13),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            text("Names are coerced to uppercase ASCII (max 8 chars). Press Enter or Apply to commit.")
                .size(11)
                .color(Color::from_rgb(0.7, 0.7, 0.75)),
            row![
                Space::new().width(Length::Fill),
                button("Apply")
                    .style(style::win32_standard_button)
                    .on_press(Message::MapNameSubmit),
            ]
            .spacing(8),
        ]
        .spacing(12);

        column![title_row, body].spacing(12).padding(16).into()
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

    /// Dedicated right-side panel hosting the 3D preview. Top-right of the
    /// viewport row, fixed width, fixed-height preview at the top, blank
    /// space below. Shares geometry + camera state with full 3D mode.
    fn view3d_side_panel(&self) -> Element<'_, Message> {
        const PANEL_W: f32 = 300.0;
        const PREVIEW_H: f32 = 240.0;
        let textures = match &self.textures {
            Some(t) => t.clone(),
            None => Arc::new(TextureSet::empty(Vec::new())),
        };
        let view = View3D {
            geometry: self.geometry3d.clone(),
            textures,
            camera: self.camera3d,
        };
        let inner = view.into_widget(Message::View3D);
        let header = container(text("3D Preview").size(11))
            .padding([2, 6])
            .style(style::win32_status_bar)
            .width(Length::Fill);
        // Wrap the shader widget in a contrasting backdrop so the brown
        // walls/floors stand out and the viewport reads as a distinct area.
        let viewport_3d = container(inner)
            .width(Length::Fill)
            .height(Length::Fixed(PREVIEW_H - 18.0))
            .style(style::viewport_3d_bg);
        let preview_card = container(
            column![header, viewport_3d]
                .width(Length::Fill)
                .height(Length::Fixed(PREVIEW_H))
        )
        .style(style::win32_modal_panel)
        .padding(0);
        // Pin to the top of the right column with a blank space filling the
        // remaining vertical area below.
        container(
            column![preview_card, Space::new().height(Length::Fill)]
                .width(Length::Fill)
                .height(Length::Fill)
                .spacing(0),
        )
        .width(Length::Fixed(PANEL_W))
        .height(Length::Fill)
        .padding(8)
        .style(style::win32_side_panel)
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
        container(view.into_widget(Message::View3D))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::viewport_3d_bg)
            .into()
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

    // Show the actual texture lump name under the slot label. GZDoom
    // Builder does this and it's the fastest way to know which "BROWN1"
    // variant you're looking at without opening the picker. Render dim
    // and fixed-width to keep the column aligned across all three slots.
    let name_text = if is_missing { "-".to_string() } else { displayed };
    let name_label = text(name_text)
        .size(10)
        .color(Color::from_rgb(0.7, 0.7, 0.75));
    column![slot, text(label).size(11), name_label]
        .spacing(2)
        .align_x(iced::Alignment::Center)
        .into()
}

fn vertical_separator() -> Element<'static, Message> {
    // The colored rule is exactly 1 px wide; the surrounding transparent
    // container provides the breathing room so the separator reads as a
    // hairline instead of a chunky bar.
    let rule = container(Space::new().width(Length::Fixed(1.0)))
        .height(Length::Fixed(18.0))
        .style(separator_style);
    container(rule).padding([0, 6]).into()
}

// ---- Menu items -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct MenuItem(&'static str);

impl std::fmt::Display for MenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Used as a visual divider inside drop-downs. Dispatches to `Noop`.
/// `pick_list` has no native separator, so we render a row of light-shade
/// box-drawing chars whose width tracks the widest menu entry.
const SEP: MenuItem = MenuItem("──────────────────");

const FILE_MENU_ITEMS: &[MenuItem] = &[
    MenuItem("New Map (Doom)"),
    MenuItem("New Map (Hexen)"),
    SEP,
    MenuItem("Open WAD…"),
    MenuItem("Open Map in Current WAD…"),
    MenuItem("Load Resource WAD…"),
    SEP,
    MenuItem("Save Map As…"),
    SEP,
    MenuItem("Quit"),
];
const EDIT_MENU_ITEMS: &[MenuItem] = &[
    MenuItem("Undo"),
    MenuItem("Redo"),
    SEP,
    MenuItem("Select All"),
    MenuItem("Clear Selection"),
    SEP,
    MenuItem("Delete Selection"),
    MenuItem("Insert Thing"),
    SEP,
    MenuItem("Copy"),
    MenuItem("Cut"),
    MenuItem("Paste"),
    MenuItem("Paste Properties"),
    SEP,
    MenuItem("Rotate Selection 90\u{B0}"),
    MenuItem("Flip Selection Horizontal"),
    MenuItem("Flip Selection Vertical"),
    SEP,
    MenuItem("Make Sector"),
    MenuItem("Split Linedefs"),
    MenuItem("Merge Vertices"),
    MenuItem("Flip Linedefs"),
    MenuItem("Flip Sidedefs"),
    MenuItem("Align Linedefs"),
    MenuItem("Stitch Overlapping Lines"),
    SEP,
    MenuItem("Auto-align Textures (X)"),
    MenuItem("Auto-align Textures (Y)"),
    MenuItem("Auto-align Textures (X+Y)"),
    SEP,
    MenuItem("Align Things to Nearest Line"),
    MenuItem("Point Things to Cursor"),
    SEP,
    MenuItem("Brightness Gradient"),
    MenuItem("Floor Gradient"),
    MenuItem("Ceiling Gradient"),
    SEP,
    MenuItem("Join Sectors"),
    MenuItem("Merge Sectors"),
    MenuItem("Make Door"),
    SEP,
    MenuItem("Snap Selection to Grid"),
    MenuItem("Increase Grid Size  ["),
    MenuItem("Decrease Grid Size  ]"),
    SEP,
    MenuItem("Map Options\u{2026}  (F2)"),
    SEP,
    MenuItem("Toggle Draw Mode"),
    MenuItem("Rectangle Draw"),
    MenuItem("Ellipse Draw"),
    MenuItem("Curve Draw"),
    MenuItem("Grid Draw"),
    MenuItem("Curve Selected Lines"),
];
const VIEW_MENU_ITEMS: &[MenuItem] = &[
    MenuItem("2D Mode"),
    MenuItem("3D Mode"),
    SEP,
    MenuItem("Fit to Screen"),
    MenuItem("Go To Coordinates\u{2026}"),
    SEP,
    MenuItem("View: Floor Textures"),
    MenuItem("View: Ceiling Textures"),
    MenuItem("View: Brightness Levels"),
    MenuItem("View: Wireframe"),
    SEP,
    MenuItem("Toggle Full Brightness"),
    MenuItem("Toggle 3D Preview"),
    MenuItem("Toggle Highlights"),
    MenuItem("Place Visual Camera Here"),
    SEP,
    MenuItem("Toggle Render Grid  (Alt+G)"),
    MenuItem("Toggle Render Things"),
    MenuItem("Toggle Render Sprites"),
    MenuItem("Toggle Render Textures"),
    MenuItem("Toggle Always Show Vertices"),
    SEP,
    MenuItem("Settings\u{2026}"),
];
const TOOLS_MENU_ITEMS: &[MenuItem] = &[
    MenuItem("Map Statistics\u{2026}"),
    MenuItem("Map Analysis\u{2026}"),
    MenuItem("Show Errors / Warnings\u{2026}"),
    SEP,
    MenuItem("View Used Tags\u{2026}"),
    MenuItem("Tag Range\u{2026}"),
    MenuItem("View Thing Types\u{2026}"),
    SEP,
    MenuItem("Reload Resources"),
    SEP,
    MenuItem("Test Map at Cursor"),
];
const MODE_MENU_ITEMS: &[MenuItem] = &[
    MenuItem("Vertices Mode  (V)"),
    MenuItem("Linedefs Mode  (L)"),
    MenuItem("Sectors Mode  (S)"),
    MenuItem("Things Mode  (T)"),
    SEP,
    MenuItem("2D Mode"),
    MenuItem("Visual Mode  (Q)"),
    SEP,
    MenuItem("Draw Lines Mode  (D)"),
    MenuItem("Draw Rectangle Mode  (\u{2318}\u{21E7}D)"),
    MenuItem("Draw Ellipse Mode  (\u{2325}\u{21E7}D)"),
    MenuItem("Draw Curve Mode  (\u{2318}\u{2325}D)"),
    MenuItem("Draw Grid Mode"),
];
const HELP_MENU_ITEMS: &[MenuItem] = &[
    MenuItem("Open Config Folder"),
    SEP,
    MenuItem("About DoomBuilder"),
];

fn dispatch_file(item: MenuItem) -> Message {
    match item.0 {
        "New Map (Doom)" => Message::NewMap(MapFormat::Doom),
        "New Map (Hexen)" => Message::NewMap(MapFormat::Hexen),
        "Open WAD…" => Message::OpenWadRequested,
        "Open Map in Current WAD…" => Message::OpenMapInWad,
        "Load Resource WAD…" => Message::LoadResourcesRequested,
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
        "Copy" => Message::CopySelection,
        "Cut" => Message::CutSelection,
        "Paste" => Message::PasteSelection,
        "Paste Properties" => Message::PasteProperties,
        "Rotate Selection 90\u{B0}" => Message::RotateSelection90,
        "Flip Selection Horizontal" => Message::FlipSelectionHorizontal,
        "Flip Selection Vertical" => Message::FlipSelectionVertical,
        "Make Sector" => Message::MakeSector,
        "Split Linedefs" => Message::SplitLines,
        "Merge Vertices" => Message::MergeVertices,
        "Flip Linedefs" => Message::FlipLines,
        "Flip Sidedefs" => Message::FlipSidedefs,
        "Align Linedefs" => Message::AlignLinedefs,
        "Stitch Overlapping Lines" => Message::StitchLines,
        "Auto-align Textures (X)" => Message::AutoAlignX,
        "Auto-align Textures (Y)" => Message::AutoAlignY,
        "Auto-align Textures (X+Y)" => Message::AutoAlignBoth,
        "Align Things to Nearest Line" => Message::AlignThingsToNearestLine,
        "Point Things to Cursor" => Message::PointThingsToCursor,
        "Snap Selection to Grid" => Message::SnapSelectionToGrid,
        "Increase Grid Size  [" => Message::CycleGridStep(1),
        "Decrease Grid Size  ]" => Message::CycleGridStep(-1),
        "Map Options…  (F2)" => Message::OpenMapOptions,
        "Brightness Gradient" => Message::MakeBrightnessGradient,
        "Floor Gradient" => Message::MakeFloorGradient,
        "Ceiling Gradient" => Message::MakeCeilingGradient,
        "Join Sectors" => Message::JoinSectors,
        "Merge Sectors" => Message::MergeSectors,
        "Make Door" => Message::MakeDoor,
        "Rectangle Draw" => Message::StartRectangleDraw,
        "Ellipse Draw" => Message::StartEllipseDraw,
        "Curve Draw" => Message::StartCurveDraw,
        "Grid Draw" => Message::StartGridDraw,
        "Curve Selected Lines" => Message::CurveSelectedLines,
        "Toggle Draw Mode" => Message::ToggleDrawing,
        _ => Message::Noop,
    }
}

fn dispatch_view(item: MenuItem) -> Message {
    match item.0 {
        "2D Mode" => Message::Mode(Mode::View2D),
        "3D Mode" => Message::Mode(Mode::View3D),
        "Fit to Screen" => Message::FitToScreen,
        "Go To Coordinates\u{2026}" => Message::OpenGoToCoords,
        "View: Floor Textures" => Message::SetView2DMode(View2DMode::Floor),
        "View: Ceiling Textures" => Message::SetView2DMode(View2DMode::Ceiling),
        "View: Brightness Levels" => Message::SetView2DMode(View2DMode::Brightness),
        "View: Wireframe" => Message::SetView2DMode(View2DMode::Wireframe),
        "Toggle Full Brightness" => Message::ToggleFullBrightness,
        "Toggle 3D Preview" => Message::Toggle3DOverlay,
        "Toggle Highlights" => Message::ToggleHighlights,
        "Place Visual Camera Here" => Message::PlaceVisualCamera,
        "Toggle Render Grid  (Alt+G)" => Message::ToggleSetting(SettingKey::ShowGrid),
        "Toggle Render Things" => Message::ToggleSetting(SettingKey::ShowThings),
        "Toggle Render Sprites" => Message::ToggleSetting(SettingKey::ShowSprites),
        "Toggle Render Textures" => Message::ToggleSetting(SettingKey::ShowTextures),
        "Toggle Always Show Vertices" => Message::ToggleSetting(SettingKey::AlwaysShowVertices),
        "Settings\u{2026}" => Message::OpenSettings,
        _ => Message::Noop,
    }
}

fn dispatch_tools(item: MenuItem) -> Message {
    match item.0 {
        "Map Statistics\u{2026}" => Message::OpenMapStats,
        "Map Analysis\u{2026}" => Message::OpenMapAnalysis,
        "Show Errors / Warnings\u{2026}" => Message::OpenMapAnalysis,
        "View Used Tags\u{2026}" => Message::OpenUsedTags,
        "Tag Range\u{2026}" => Message::OpenTagRange,
        "View Thing Types\u{2026}" => Message::OpenThingTypes,
        "Test Map at Cursor" => Message::TestMapAtCursor,
        "Reload Resources" => Message::ReloadResources,
        _ => Message::Noop,
    }
}

fn dispatch_mode(item: MenuItem) -> Message {
    match item.0 {
        "Vertices Mode  (V)" => Message::SetEditMode(EditMode::Vertices),
        "Linedefs Mode  (L)" => Message::SetEditMode(EditMode::Linedefs),
        "Sectors Mode  (S)" => Message::SetEditMode(EditMode::Sectors),
        "Things Mode  (T)" => Message::SetEditMode(EditMode::Things),
        "2D Mode" => Message::Mode(Mode::View2D),
        "Visual Mode  (Q)" => Message::Mode(Mode::View3D),
        "Draw Lines Mode  (D)" => Message::ToggleDrawing,
        "Draw Rectangle Mode  (⌘⇧D)" => Message::StartRectangleDraw,
        "Draw Ellipse Mode  (⌥⇧D)" => Message::StartEllipseDraw,
        "Draw Curve Mode  (⌘⌥D)" => Message::StartCurveDraw,
        "Draw Grid Mode" => Message::StartGridDraw,
        _ => Message::Noop,
    }
}

fn dispatch_help(item: MenuItem) -> Message {
    match item.0 {
        "Open Config Folder" => Message::OpenConfigFolder,
        "About DoomBuilder" => Message::OpenAboutDialog,
        _ => Message::Noop,
    }
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

// ── Settings panel helpers ──────────────────────────────────────────────
//
// Card-based layout: the modal is a vertical stack of grouped cards, each
// with a title, dim sublabel, and content. Path rows show a colored pip
// (green when set, red when missing) so the user can scan the page and see
// what still needs attention.

fn settings_header_style(_theme: &Theme) -> container::Style {
    let p = palette::active();
    container::Style {
        text_color: Some(p.text),
        background: Some(iced::Background::Color(p.elevated)),
        border: iced::Border {
            color: p.border,
            width: 0.0,
            radius: iced::border::Radius::new(0.0)
                .top_left(16.0)
                .top_right(16.0),
        },
        shadow: iced::Shadow::default(),
        snap: true,
    }
}

fn settings_card_style(_theme: &Theme) -> container::Style {
    let p = palette::active();
    container::Style {
        text_color: Some(p.text),
        background: Some(iced::Background::Color(p.elevated)),
        border: iced::Border {
            color: p.border,
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: iced::Shadow::default(),
        snap: true,
    }
}

fn pip_ok_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgb(
            0.30, 0.78, 0.45,
        ))),
        border: iced::Border {
            color: iced::Color::TRANSPARENT,
            width: 0.0,
            radius: 100.0.into(),
        },
        ..Default::default()
    }
}

fn pip_warn_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgb(
            0.92, 0.46, 0.40,
        ))),
        border: iced::Border {
            color: iced::Color::TRANSPARENT,
            width: 0.0,
            radius: 100.0.into(),
        },
        ..Default::default()
    }
}

fn settings_card<'a>(
    title: &'a str,
    subtitle: &'a str,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    let header = column![
        text(title).size(15),
        text(subtitle).size(11).color(palette::active().text_dim),
    ]
    .spacing(2);
    container(column![header, body].spacing(12))
        .padding(16)
        .width(Length::Fill)
        .style(settings_card_style)
        .into()
}

fn labelled_field<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    column![
        text(label).size(12).color(palette::active().text_dim),
        control,
    ]
    .spacing(4)
    .into()
}

fn toggle_row<'a>(app: &App, key: SettingKey) -> Element<'a, Message> {
    let on = key.get(&app.settings);
    checkbox(on)
        .label(key.label())
        .on_toggle(move |v| Message::SetSetting(key, v))
        .into()
}

fn status_pip<'a>(ok: bool) -> Element<'a, Message> {
    container(Space::new())
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(if ok { pip_ok_style } else { pip_warn_style })
        .into()
}

fn path_field<'a>(
    label: &'a str,
    path: Option<&'a PathBuf>,
    is_set: bool,
    on_pick: Message,
) -> Element<'a, Message> {
    let value: String = path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Not set".into());
    let value_color = if is_set {
        palette::active().text
    } else {
        palette::active().text_dim
    };
    column![
        text(label).size(12).color(palette::active().text_dim),
        row![
            status_pip(is_set),
            text(value).size(13).color(value_color),
            Space::new().width(Length::Fill),
            button("Choose\u{2026}")
                .style(style::win32_standard_button)
                .on_press(on_pick),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(4)
    .into()
}

async fn pick_save_path(suggested_stem: String) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_file_name(format!("{suggested_stem}.wad"))
        .add_filter("PWAD", &["wad"])
        .save_file()
        .await
        .map(|h| h.path().to_path_buf())
}

async fn save_map_to_path(
    map: Arc<Map>,
    path: PathBuf,
    builder: NodeBuilder,
) -> Result<PathBuf, String> {
    // Builtin path stays infallible; external builders surface their error.
    let bytes = match builder {
        NodeBuilder::Builtin => save_map_as_pwad(&map),
        b @ NodeBuilder::Zdbsp { .. } => save_map_as_pwad_with(&map, &b).map_err(|e| e.to_string())?,
    };
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Convert a 2D math-Y-up direction vector to a Doom angle in degrees
/// (0 = East, 90 = North, 180 = West, 270 = South), normalized to 0..360.
fn doom_angle_of(dx: f32, dy: f32) -> i32 {
    let deg = (dy.atan2(dx).to_degrees()).round() as i32;
    deg.rem_euclid(360)
}

/// Map a sector's light value (0..255) to a 32-bit RGBA color for the
/// "View Brightness Levels" mode. Black at 0, warm white at 255.
fn brightness_color(light: i16) -> [u8; 4] {
    let l = (light.clamp(0, 255) as f32) / 255.0;
    let r = (l * 255.0) as u8;
    let g = (l * 245.0) as u8;
    let b = (l * 215.0) as u8;
    [r, g, b, 255]
}

// ---- Shape vertex builders --------------------------------------------------

fn rect_corners(a: Vec2, b: Vec2) -> (Vec2, Vec2) {
    let min = Vec2::new(a.x.min(b.x), a.y.min(b.y));
    let max = Vec2::new(a.x.max(b.x), a.y.max(b.y));
    (min, max)
}

/// Vertices of a rectangle from `a` to `b`. When `bevel > 0`, each corner is
/// chamfered by that many world units (clamped to half the shorter side).
/// Ordered clockwise in math-Y-up coords so the front (right) sidedef of
/// each emitted linedef faces inward — matches Doom's sector convention.
fn rectangle_vertices(a: Vec2, b: Vec2, bevel: u32) -> Vec<Vec2> {
    let (min, max) = rect_corners(a, b);
    let w = max.x - min.x;
    let h = max.y - min.y;
    if w < 1.0 || h < 1.0 {
        return Vec::new();
    }
    let bv = (bevel as f32).min((w * 0.5).min(h * 0.5));
    if bv < 0.5 {
        // Plain rectangle, 4 vertices, CW (math Y-up).
        return vec![
            Vec2::new(min.x, min.y),
            Vec2::new(min.x, max.y),
            Vec2::new(max.x, max.y),
            Vec2::new(max.x, min.y),
        ];
    }
    // Beveled rectangle, 8 vertices, CW.
    vec![
        Vec2::new(min.x, min.y + bv),
        Vec2::new(min.x, max.y - bv),
        Vec2::new(min.x + bv, max.y),
        Vec2::new(max.x - bv, max.y),
        Vec2::new(max.x, max.y - bv),
        Vec2::new(max.x, min.y + bv),
        Vec2::new(max.x - bv, min.y),
        Vec2::new(min.x + bv, min.y),
    ]
}

/// N-subdivision approximation of an axis-aligned ellipse inscribed in the
/// bounding box from `a` to `b`. Walked clockwise (math Y-up) so the right
/// sidedef of each emitted linedef faces inward.
fn ellipse_vertices(a: Vec2, b: Vec2, subdivisions: u32) -> Vec<Vec2> {
    let (min, max) = rect_corners(a, b);
    let cx = (min.x + max.x) * 0.5;
    let cy = (min.y + max.y) * 0.5;
    let rx = (max.x - min.x) * 0.5;
    let ry = (max.y - min.y) * 0.5;
    if rx < 0.5 || ry < 0.5 {
        return Vec::new();
    }
    let n = subdivisions.max(4) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // Negate the sine so we sweep clockwise instead of CCW.
        let t = (i as f32 / n as f32) * std::f32::consts::TAU;
        out.push(Vec2::new(cx + rx * t.cos(), cy - ry * t.sin()));
    }
    out
}

fn quadratic_bezier_point(p0: Vec2, p1: Vec2, p2: Vec2, t: f32) -> Vec2 {
    let mt = 1.0 - t;
    Vec2::new(
        mt * mt * p0.x + 2.0 * mt * t * p1.x + t * t * p2.x,
        mt * mt * p0.y + 2.0 * mt * t * p1.y + t * t * p2.y,
    )
}

/// Sample a quadratic bezier with `subdivisions` segments → `subdivisions+1`
/// vertices spanning p0..p2 with control p1.
fn quadratic_bezier_vertices(p0: Vec2, p2: Vec2, p1: Vec2, subdivisions: u32) -> Vec<Vec2> {
    let n = subdivisions.max(2);
    let mut out = Vec::with_capacity((n + 1) as usize);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        out.push(quadratic_bezier_point(p0, p1, p2, t));
    }
    out
}

/// Construct a `LinedefChain` from a vertex polyline. `closed = true`
/// connects the last vertex back to the first.
fn chain_from_polyline(points: &[Vec2], closed: bool) -> LinedefChain {
    let mut chain = LinedefChain::default();
    for p in points {
        chain
            .vertex_inserts
            .push(doombuilder_core::map::MapVertex {
                x: p.x.round() as i32,
                y: p.y.round() as i32,
            });
    }
    let n = points.len();
    let line_count = if closed { n } else { n - 1 };
    for i in 0..line_count {
        let a = i;
        let b = (i + 1) % n;
        chain.linedefs.push((
            LineEndpoint::New(a),
            LineEndpoint::New(b),
            doombuilder_core::map::MapLinedef {
                v1: doombuilder_core::map::VertexId::default(),
                v2: doombuilder_core::map::VertexId::default(),
                flags: 0,
                special: 0,
                args: [0; 5],
                tag: 0,
                right: None,
                left: None,
                fields: Default::default(),
            },
        ));
    }
    chain
}

/// Grid of rectangles inside `a..b`. Produces (cols+1)*(rows+1) vertices and
/// (cols+1)*rows + (rows+1)*cols linedefs (horizontal + vertical grid lines).
fn chain_from_grid(a: Vec2, b: Vec2, cols: u32, rows: u32) -> LinedefChain {
    let (min, max) = rect_corners(a, b);
    let mut chain = LinedefChain::default();
    let cols = cols.max(1) as usize;
    let rows = rows.max(1) as usize;
    let nx = cols + 1;
    let ny = rows + 1;
    let dx = (max.x - min.x) / cols as f32;
    let dy = (max.y - min.y) / rows as f32;
    if dx < 1.0 || dy < 1.0 {
        return chain;
    }
    // Vertices in row-major order: (col, row).
    let idx = |c: usize, r: usize| -> usize { r * nx + c };
    for r in 0..ny {
        for c in 0..nx {
            let x = min.x + c as f32 * dx;
            let y = min.y + r as f32 * dy;
            chain
                .vertex_inserts
                .push(doombuilder_core::map::MapVertex {
                    x: x.round() as i32,
                    y: y.round() as i32,
                });
        }
    }
    let new_line = |a: usize, b: usize| {
        (
            LineEndpoint::New(a),
            LineEndpoint::New(b),
            doombuilder_core::map::MapLinedef {
                v1: doombuilder_core::map::VertexId::default(),
                v2: doombuilder_core::map::VertexId::default(),
                flags: 0,
                special: 0,
                args: [0; 5],
                tag: 0,
                right: None,
                left: None,
                fields: Default::default(),
            },
        )
    };
    // Horizontal lines across each row.
    for r in 0..ny {
        for c in 0..cols {
            chain.linedefs.push(new_line(idx(c, r), idx(c + 1, r)));
        }
    }
    // Vertical lines down each column.
    for c in 0..nx {
        for r in 0..rows {
            chain.linedefs.push(new_line(idx(c, r), idx(c, r + 1)));
        }
    }
    chain
}

/// Order `line_ids` into a vertex path. Returns `None` if the selection
/// isn't a single open chain (any vertex shared by >2 lines, or the chain
/// is disconnected). Endpoint vertices have degree 1; all others degree 2.
fn order_line_chain(map: &Map, line_ids: &[LinedefId]) -> Option<Vec<VertexId>> {
    use std::collections::HashMap;
    let mut adj: HashMap<VertexId, Vec<(VertexId, LinedefId)>> = HashMap::new();
    for lid in line_ids {
        let l = map.linedefs.get(*lid)?;
        adj.entry(l.v1).or_default().push((l.v2, *lid));
        adj.entry(l.v2).or_default().push((l.v1, *lid));
    }
    if adj.values().any(|v| v.len() > 2) {
        return None;
    }
    let endpoints: Vec<VertexId> =
        adj.iter().filter(|(_, ns)| ns.len() == 1).map(|(v, _)| *v).collect();
    if endpoints.len() != 2 {
        return None;
    }
    let mut path = vec![endpoints[0]];
    let mut visited_lines: HashSet<LinedefId> = HashSet::new();
    loop {
        let cur = *path.last().unwrap();
        let next = adj
            .get(&cur)?
            .iter()
            .find(|(_, lid)| !visited_lines.contains(lid))
            .copied();
        let Some((nxt_v, nxt_l)) = next else { break };
        visited_lines.insert(nxt_l);
        path.push(nxt_v);
        if Some(&nxt_v) == endpoints.last() && visited_lines.len() == line_ids.len() {
            break;
        }
    }
    if visited_lines.len() != line_ids.len() {
        return None;
    }
    Some(path)
}

async fn pick_file() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter("Doom assets", &["wad", "pk3", "zip"])
        .add_filter("All files", &["*"])
        .pick_file()
        .await
        .map(|h| h.path().to_path_buf())
}

/// Decode a map name into `-warp` arguments. Vanilla engines take:
///   * `-warp E M` for E#M# (Doom 1, Heretic)
///   * `-warp NN` for MAPNN (Doom 2, Hexen)
/// Returns None for unrecognised names; engine launches at title.
fn warp_args_for(name: &str) -> Option<Vec<String>> {
    let up = name.to_ascii_uppercase();
    // E#M#
    if up.len() == 4 && up.as_bytes()[0] == b'E' && up.as_bytes()[2] == b'M' {
        let e = (up.as_bytes()[1] as char).to_digit(10)?;
        let m = (up.as_bytes()[3] as char).to_digit(10)?;
        return Some(vec!["-warp".into(), e.to_string(), m.to_string()]);
    }
    // MAPNN
    if let Some(rest) = up.strip_prefix("MAP") {
        if let Ok(n) = rest.parse::<u32>() {
            return Some(vec!["-warp".into(), n.to_string()]);
        }
    }
    None
}

async fn pick_executable() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Choose Doom engine binary")
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
