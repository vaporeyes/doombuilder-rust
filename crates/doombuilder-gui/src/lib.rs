// ABOUTME: Iced application root for doombuilder-rust.
// ABOUTME: File picker, sidebar, mode toggle (2D/3D), and the 2D Canvas viewport.

mod camera;
mod view2d;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use doombuilder_core::archive::{open as open_asset, Asset, Pk3};
use doombuilder_core::map::Map;
use doombuilder_core::wad::WadKind;
use doombuilder_core::{load_auto, MapFormat, Wad};
use doombuilder_render::{
    build_walls, extract_sector_loops, triangulate_sector, FloorMesh, Wall,
};
use glam::Vec2;
use iced::widget::{button, column, container, row, scrollable, text};
use iced::widget::canvas::Cache;
use iced::{Element, Length, Task};

use camera::Camera2D;
use view2d::{map_aabb, View2D, View2DMessage};

pub fn run() -> iced::Result {
    iced::application(App::default, App::update, App::view)
        .title("DoomBuilder")
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
    summary: Option<String>,
    maps: Vec<String>,
    selected_map: Option<String>,
    map: Option<Arc<Map>>,
    map_stats: Option<MapStats>,
    sector_meshes: Arc<Vec<FloorMesh>>,
    walls: Arc<Vec<Wall>>,
    camera2d: Camera2D,
    cache2d: Arc<Cache>,
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
    stats: MapStats,
}

impl App {
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
                self.wad = asset.wad;
                self.summary = Some(asset.summary);
                self.maps = asset.maps;
                self.selected_map = None;
                self.map = None;
                self.map_stats = None;
                self.sector_meshes = Arc::new(Vec::new());
                self.walls = Arc::new(Vec::new());
                self.cache2d = Arc::new(Cache::new());
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
                let s = &payload.stats;
                let fmt = match s.format {
                    MapFormat::Doom => "Doom",
                    MapFormat::Hexen => "Hexen",
                };
                self.status = format!(
                    "{} ({fmt}): {} verts, {} lines, {} sides, {} sectors, {} things",
                    s.name, s.vertices, s.linedefs, s.sidedefs, s.sectors, s.things
                );
                self.map_stats = Some(payload.stats);
                self.map = Some(payload.map.clone());
                self.sector_meshes = payload.sector_meshes;
                self.walls = payload.walls;
                self.cache2d = Arc::new(Cache::new());
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

    fn handle_view2d(&mut self, msg: View2DMessage) {
        match msg {
            View2DMessage::PanBy(delta) => self.camera2d.pan_screen(delta),
            View2DMessage::ZoomAt {
                pivot,
                factor,
                viewport,
            } => self.camera2d.zoom_about(pivot, viewport, factor),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let menu = row![
            button("Open WAD...").on_press(Message::OpenWadRequested),
            mode_button("2D", Mode::View2D, self.mode),
            mode_button("3D", Mode::View3D, self.mode),
            button("Quit").on_press(Message::Quit),
        ]
        .spacing(8)
        .padding(8);

        let body = row![self.sidebar(), self.viewport()].height(Length::Fill);
        let status_bar = container(text(&self.status)).width(Length::Fill).padding(6);
        column![menu, body, status_bar].into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let header: Element<'_, Message> = match &self.summary {
            Some(s) => text(s).size(14).into(),
            None => text("No asset loaded.").into(),
        };

        let maps_list: Element<'_, Message> = if self.maps.is_empty() {
            text("(no maps)").into()
        } else {
            let rows: Vec<Element<'_, Message>> = self
                .maps
                .iter()
                .map(|name| {
                    let is_selected = self.selected_map.as_deref() == Some(name.as_str());
                    let label = if is_selected {
                        format!("> {name}")
                    } else {
                        name.clone()
                    };
                    button(text(label))
                        .on_press(Message::MapSelected(name.clone()))
                        .width(Length::Fill)
                        .into()
                })
                .collect();
            scrollable(column(rows).spacing(2))
                .height(Length::Fill)
                .into()
        };

        container(column![header, maps_list].spacing(8).padding(8))
            .width(Length::Fixed(200.0))
            .height(Length::Fill)
            .into()
    }

    fn viewport(&self) -> Element<'_, Message> {
        match (self.mode, &self.map) {
            (_, None) => container(text("Select a map to load."))
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
                };
                view.into_widget(Message::View2D)
            }
            (Mode::View3D, Some(_)) => self.view3d_placeholder(),
        }
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

fn mode_button(label: &str, target: Mode, current: Mode) -> Element<'static, Message> {
    let mut b = button(text(label.to_string()));
    if target != current {
        b = b.on_press(Message::Mode(target));
    }
    b.into()
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

    // Triangulate every sector. Failures are silently dropped so a single
    // malformed sector cannot prevent the rest of the map from rendering.
    let loops_by_sector = extract_sector_loops(&map);
    let mut meshes = Vec::with_capacity(loops_by_sector.len());
    let mut triangulated = 0usize;
    for (sid, loops) in &loops_by_sector {
        match triangulate_sector(&map, *sid, loops) {
            Ok(mesh) if !mesh.indices.is_empty() => {
                triangulated += 1;
                meshes.push(mesh);
            }
            _ => {}
        }
    }

    let walls = build_walls(&map);

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
