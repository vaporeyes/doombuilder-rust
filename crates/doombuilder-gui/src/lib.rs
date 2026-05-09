// ABOUTME: Iced application root for doombuilder-rust.
// ABOUTME: UDB-style layout: dynamic title, toolbar with map picker, full
// ABOUTME: viewport, bottom inspector with texture slots, status bar.

mod camera;
mod view2d;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use doombuilder_core::archive::{open as open_asset, Asset, Pk3};
use doombuilder_core::map::{Map, MapSidedef, SectorId, TextureName};
use doombuilder_core::wad::WadKind;
use doombuilder_core::{load_auto, MapFormat, Wad};
use doombuilder_render::{
    build_walls, extract_sector_loops, triangulate_sector, FloorMesh, SpatialIndex, Wall,
};
use glam::Vec2;
use iced::widget::canvas::Cache;
use iced::widget::{button, column, container, pick_list, row, text, Space};
use iced::{Border, Color, Element, Length, Task, Theme};

use camera::Camera2D;
use view2d::{map_aabb, HighlightKind, View2D, View2DMessage};

pub fn run() -> iced::Result {
    iced::application(App::default, App::update, App::view)
        .title(App::window_title)
        .run()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    View2D,
    View3D,
}

#[derive(Default)]
pub struct App {
    status: String,
    wad: Option<Wad>,
    wad_path: Option<PathBuf>,
    summary: Option<String>,
    maps: Vec<String>,
    selected_map: Option<String>,
    map: Option<Arc<Map>>,
    map_stats: Option<MapStats>,
    sector_meshes: Arc<Vec<FloorMesh>>,
    walls: Arc<Vec<Wall>>,
    spatial: Option<Arc<SpatialIndex>>,
    camera2d: Camera2D,
    cache2d: Arc<Cache>,
    hover: Option<HighlightKind>,
    selection: Option<HighlightKind>,
    mode: Mode,
}

#[derive(Debug, Clone)]
pub enum Message {
    OpenWadRequested,
    FilePicked(Option<PathBuf>),
    AssetLoaded(Result<AssetSummary, String>),
    MapSelected(String),
    MapLoaded(Result<MapPayload, String>),
    Mode(Mode),
    View2D(View2DMessage),
    Quit,
}

#[derive(Debug, Clone)]
pub struct AssetSummary {
    path: PathBuf,
    wad: Option<Wad>,
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
    sector_meshes: usize,
    walls: usize,
}

#[derive(Debug, Clone)]
pub struct MapPayload {
    map: Arc<Map>,
    sector_meshes: Arc<Vec<FloorMesh>>,
    walls: Arc<Vec<Wall>>,
    spatial: Arc<SpatialIndex>,
    stats: MapStats,
}

impl App {
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
        match message {
            Message::OpenWadRequested => {
                self.status = "Choose a WAD, PK3, or zip...".to_string();
                Task::perform(pick_file(), Message::FilePicked)
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
                self.selection = None;
                if let Some((min, max)) = map_aabb(&payload.map) {
                    self.camera2d.frame_aabb(min, max, Vec2::new(800.0, 600.0));
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
            Message::View2D(msg) => {
                self.handle_view2d(msg);
                self.cache2d.clear();
                Task::none()
            }
            Message::Quit => iced::exit(),
        }
    }

    fn reset_map_state(&mut self) {
        self.selected_map = None;
        self.map = None;
        self.map_stats = None;
        self.sector_meshes = Arc::new(Vec::new());
        self.walls = Arc::new(Vec::new());
        self.spatial = None;
        self.cache2d = Arc::new(Cache::new());
        self.hover = None;
        self.selection = None;
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
                let new_hover = self.hit_test(world);
                if new_hover != self.hover {
                    self.hover = new_hover;
                }
            }
            View2DMessage::HoverCleared => {
                self.hover = None;
            }
            View2DMessage::ClickAt(world) => {
                self.selection = self.hit_test(world);
            }
        }
    }

    fn hit_test(&self, world: Vec2) -> Option<HighlightKind> {
        let spatial = self.spatial.as_ref()?;
        let zoom = self.camera2d.zoom.max(1e-6);
        let vertex_radius = 8.0 / zoom;
        let linedef_radius = 5.0 / zoom;
        spatial
            .hit_test(world.x, world.y, vertex_radius, linedef_radius)
            .map(HighlightKind::from)
    }

    fn view(&self) -> Element<'_, Message> {
        let toolbar = self.toolbar();
        let viewport = self.viewport_widget();
        let mut layout = column![toolbar, viewport].spacing(0);
        if let Some(panel) = self.bottom_panel() {
            layout = layout.push(panel);
        }
        layout = layout.push(self.status_bar());
        layout.into()
    }

    fn toolbar(&self) -> Element<'_, Message> {
        let map_picker: Element<'_, Message> = if self.maps.is_empty() {
            text("No map").size(14).into()
        } else {
            pick_list(
                self.maps.clone(),
                self.selected_map.clone(),
                Message::MapSelected,
            )
            .placeholder("Pick map…")
            .into()
        };

        container(
            row![
                button("Open WAD...").on_press(Message::OpenWadRequested),
                vertical_separator(),
                text("Map:").size(14),
                map_picker,
                vertical_separator(),
                mode_button("2D", Mode::View2D, self.mode),
                mode_button("3D", Mode::View3D, self.mode),
                Space::new().width(Length::Fill),
                button("Quit").on_press(Message::Quit),
            ]
            .spacing(8)
            .padding(8)
            .align_y(iced::Alignment::Center),
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
                    selection: self.selection,
                };
                view.into_widget(Message::View2D)
            }
            (Mode::View3D, Some(_)) => self.view3d_placeholder(),
        };
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn bottom_panel(&self) -> Option<Element<'_, Message>> {
        let map = self.map.as_ref()?;

        let details_body: Element<'_, Message> = match self.selection {
            Some(highlight) => selection_details(map, highlight),
            None => column![
                text("No selection").size(15),
                text("Click an element to inspect.").size(12),
            ]
            .spacing(2)
            .into(),
        };
        let details = container(details_body)
            .width(Length::Fixed(320.0))
            .padding(10);

        let (front_side, back_side) = match self.selection {
            Some(HighlightKind::Linedef(id)) => {
                let line = map.linedefs.get(id);
                (
                    line.and_then(|l| l.right)
                        .and_then(|sid| map.sidedefs.get(sid)),
                    line.and_then(|l| l.left)
                        .and_then(|sid| map.sidedefs.get(sid)),
                )
            }
            _ => (None, None),
        };
        let texture_panels = row![
            side_panel(map, "Front Side", front_side),
            side_panel(map, "Back Side", back_side),
        ]
        .spacing(10);

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
                    "{fmt} | {} verts | {} lines | {} sides | {} sectors | {} things",
                    s.vertices, s.linedefs, s.sidedefs, s.sectors, s.things
                )
            })
            .unwrap_or_else(|| self.status.clone());
        let grid = self.camera2d.grid_step();
        let zoom = self.camera2d.zoom;
        let right = if self.map.is_some() {
            format!("Grid: {grid:.0}   Zoom: {zoom:.3}")
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

    fn view3d_placeholder(&self) -> Element<'_, Message> {
        let stats = self
            .map_stats
            .as_ref()
            .map(|s| {
                format!(
                    "3D data ready:\n  triangulated sectors: {}\n  wall quads: {}\n\n(wgpu pipeline lands next session)",
                    s.sector_meshes, s.walls
                )
            })
            .unwrap_or_else(|| "Load a map first.".into());
        container(text(stats).size(14))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }
}

fn selection_details<'a>(map: &Map, highlight: HighlightKind) -> Element<'a, Message> {
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
            let mut col = column![text(format!("Linedef {:?}", id)).size(15)].spacing(2);
            if let Some(l) = map.linedefs.get(id) {
                let length = match (map.vertices.get(l.v1), map.vertices.get(l.v2)) {
                    (Some(a), Some(b)) => {
                        let dx = (b.x - a.x) as f32;
                        let dy = (b.y - a.y) as f32;
                        (dx * dx + dy * dy).sqrt()
                    }
                    _ => 0.0,
                };
                col = col.push(text(format!("Action:  {}", l.special)));
                col = col.push(text(format!("Length:  {length:.0}")));
                col = col.push(text(format!("Tag:     {}", l.tag)));
                col = col.push(text(format!("Flags:   0x{:04X}", l.flags)));
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
        HighlightKind::Sector(id) => {
            let mut col = column![text(format!("Sector {:?}", id)).size(15)].spacing(2);
            if let Some(s) = map.sectors.get(id) {
                col = col.push(text(format!("Floor:   {}", s.floor_height)));
                col = col.push(text(format!("Ceiling: {}", s.ceiling_height)));
                col = col.push(text(format!("Light:   {}", s.light)));
                col = col.push(text(format!("Special: {}", s.special)));
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

fn side_panel<'a>(_map: &Map, title: &'a str, side: Option<&MapSidedef>) -> Element<'a, Message> {
    let slots: Element<'_, Message> = match side {
        Some(side) => row![
            texture_slot("Upper", side.upper_texture),
            texture_slot("Middle", side.middle_texture),
            texture_slot("Lower", side.lower_texture),
        ]
        .spacing(8)
        .into(),
        None => text("(none)").size(13).into(),
    };
    container(
        column![text(title).size(14), slots]
            .spacing(6),
    )
    .padding(8)
    .style(side_panel_style)
    .into()
}

fn texture_slot<'a>(label: &'a str, name: TextureName) -> Element<'a, Message> {
    let displayed = name.as_str();
    let (line, color) = if displayed.is_empty() || displayed == "-" {
        ("Missing".to_string(), Color::from_rgb(0.85, 0.45, 0.45))
    } else {
        (displayed.to_string(), Color::from_rgb(0.9, 0.9, 0.9))
    };
    column![
        container(
            text(line)
                .size(12)
                .color(color)
        )
        .width(Length::Fixed(72.0))
        .height(Length::Fixed(72.0))
        .center_x(Length::Fixed(72.0))
        .center_y(Length::Fixed(72.0))
        .style(texture_slot_style),
        text(label).size(11),
    ]
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

fn mode_button(label: &str, target: Mode, current: Mode) -> Element<'static, Message> {
    let mut b = button(text(label.to_string()));
    if target != current {
        b = b.on_press(Message::Mode(target));
    }
    b.into()
}

fn panel_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.13, 0.13, 0.15))),
        border: Border {
            color: Color::from_rgb(0.05, 0.05, 0.06),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn status_bar_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.10, 0.10, 0.12))),
        text_color: Some(Color::from_rgb(0.7, 0.7, 0.75)),
        ..Default::default()
    }
}

fn side_panel_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.16, 0.16, 0.18))),
        border: Border {
            color: Color::from_rgb(0.05, 0.05, 0.06),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

fn texture_slot_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.08, 0.08, 0.10))),
        border: Border {
            color: Color::from_rgb(0.30, 0.30, 0.32),
            width: 1.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

fn separator_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.30, 0.30, 0.34))),
        ..Default::default()
    }
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
    open_and_summarise(&path).map_err(|e| e.to_string()).map(
        |(wad, summary, maps)| AssetSummary {
            path,
            wad,
            summary,
            maps,
        },
    )
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
    let triangulated = meshes_with_id.len();

    let walls = build_walls(&map);
    let spatial = SpatialIndex::build(&map, meshes_with_id.clone());

    let meshes: Vec<FloorMesh> = meshes_with_id.into_iter().map(|(_, m)| m).collect();

    let stats = MapStats {
        name: map.name.clone(),
        format: map.format,
        vertices: map.vertices.len(),
        linedefs: map.linedefs.len(),
        sidedefs: map.sidedefs.len(),
        sectors: map.sectors.len(),
        things: map.things.len(),
        sector_meshes: triangulated,
        walls: walls.len(),
    };

    Ok(MapPayload {
        map: Arc::new(map),
        sector_meshes: Arc::new(meshes),
        walls: Arc::new(walls),
        spatial: Arc::new(spatial),
        stats,
    })
}

fn open_and_summarise(
    path: &Path,
) -> Result<(Option<Wad>, String, Vec<String>), doombuilder_core::Error> {
    match open_asset(path)? {
        Asset::Wad(wad) => {
            let (summary, maps) = summarise_wad(&wad);
            Ok((Some(wad), summary, maps))
        }
        Asset::Pk3(pk3) => {
            let (summary, maps) = summarise_pk3(&pk3);
            Ok((None, summary, maps))
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
