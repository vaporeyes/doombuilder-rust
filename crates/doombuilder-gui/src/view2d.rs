// ABOUTME: 2D editor viewport using iced's Canvas. Renders grid, filled sectors,
// ABOUTME: linedefs, vertices, and hover/selection overlays. Mouse: middle/right
// ABOUTME: drag pans, wheel zooms about cursor, left click selects, move hovers.

use std::collections::HashSet;
use std::sync::Arc;

use doombuilder_core::config::GameConfig;
use doombuilder_core::map::{LinedefId, Map, SectorId, ThingId, VertexId};

use crate::EditMode;
use doombuilder_render::{FloorMesh, Hit};
use glam::Vec2;
use iced::mouse;
use iced::widget::canvas::{self, Cache, Canvas, Event, Frame, Geometry, Path, Program, Stroke};
use iced::widget::image::Handle as ImageHandle;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

use crate::camera::Camera2D;

const DRAG_THRESHOLD_PX: f32 = 3.0;

#[derive(Debug, Clone)]
pub enum View2DMessage {
    PanBy(Vec2),
    ZoomAt { pivot: Vec2, factor: f32, viewport: Vec2 },
    HoverAt(Vec2),
    HoverCleared,
    ClickAt(Vec2),
    DragMoved { start: Vec2, current: Vec2 },
    DragComplete { start: Vec2, end: Vec2 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HighlightKind {
    Vertex(VertexId),
    Linedef(LinedefId),
    Sector(SectorId),
    Thing(ThingId),
}

impl From<Hit> for HighlightKind {
    fn from(h: Hit) -> Self {
        match h {
            Hit::Vertex(v) => HighlightKind::Vertex(v),
            Hit::Linedef(l) => HighlightKind::Linedef(l),
            Hit::Sector(s) => HighlightKind::Sector(s),
            Hit::Thing(t) => HighlightKind::Thing(t),
        }
    }
}

#[derive(Clone)]
pub struct FillTile {
    pub handle: ImageHandle,
    pub world_min: Vec2,
    pub world_max: Vec2,
}

pub struct View2D {
    pub map: Arc<Map>,
    pub meshes: Arc<Vec<(SectorId, FloorMesh)>>,
    pub camera: Camera2D,
    pub cache: Arc<Cache>,
    pub hover: Option<HighlightKind>,
    pub selection: Arc<HashSet<HighlightKind>>,
    pub drag_rect: Option<(Vec2, Vec2)>,
    pub fills: Arc<Vec<FillTile>>,
    pub config: Arc<GameConfig>,
    pub edit_mode: EditMode,
}

impl View2D {
    pub fn into_widget<Message: 'static>(
        self,
        on_event: impl Fn(View2DMessage) -> Message + 'static,
    ) -> Element<'static, Message>
    where
        Message: Clone,
    {
        Canvas::new(View2DProgram {
            inner: self,
            on_event: Box::new(on_event),
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

struct View2DProgram<Message> {
    inner: View2D,
    on_event: Box<dyn Fn(View2DMessage) -> Message>,
}

#[derive(Default)]
pub struct InternalState {
    panning: bool,
    last_cursor: Option<Point>,
    cursor_in_bounds: bool,
    drag_start: Option<Point>,
    drag_active: bool,
}

impl<Message> Program<Message> for View2DProgram<Message> {
    type State = InternalState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let viewport = Vec2::new(bounds.width, bounds.height);
        let cursor_pos = cursor.position_in(bounds);

        match event {
            Event::Mouse(mouse::Event::CursorLeft) => {
                state.cursor_in_bounds = false;
                state.panning = false;
                state.drag_start = None;
                state.drag_active = false;
                return Some(
                    canvas::Action::publish((self.on_event)(View2DMessage::HoverCleared))
                        .and_capture(),
                );
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle))
            | Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                if let Some(p) = cursor_pos {
                    state.panning = true;
                    state.last_cursor = Some(p);
                    return Some(canvas::Action::request_redraw().and_capture());
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Middle))
            | Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Right)) => {
                state.panning = false;
                state.last_cursor = None;
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor_pos {
                    state.drag_start = Some(p);
                    state.drag_active = false;
                    return Some(canvas::Action::request_redraw().and_capture());
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let Some(p) = cursor_pos else {
                    state.drag_start = None;
                    state.drag_active = false;
                    return None;
                };
                let camera = &self.inner.camera;
                let was_drag = state.drag_active;
                let start = state.drag_start.take();
                state.drag_active = false;
                let msg = match (was_drag, start) {
                    (true, Some(s)) => {
                        let start_w = camera.screen_to_world(Vec2::new(s.x, s.y), viewport);
                        let end_w = camera.screen_to_world(Vec2::new(p.x, p.y), viewport);
                        View2DMessage::DragComplete {
                            start: start_w,
                            end: end_w,
                        }
                    }
                    _ => {
                        let world = camera.screen_to_world(Vec2::new(p.x, p.y), viewport);
                        View2DMessage::ClickAt(world)
                    }
                };
                return Some(canvas::Action::publish((self.on_event)(msg)).and_capture());
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(p) = cursor_pos {
                    state.cursor_in_bounds = true;
                    let prev = state.last_cursor.replace(p);
                    if state.panning {
                        if let Some(prev) = prev {
                            let delta = Vec2::new(p.x - prev.x, p.y - prev.y);
                            return Some(
                                canvas::Action::publish((self.on_event)(View2DMessage::PanBy(delta)))
                                    .and_capture(),
                            );
                        }
                    } else if let Some(start) = state.drag_start {
                        let dx = p.x - start.x;
                        let dy = p.y - start.y;
                        let dist_sq = dx * dx + dy * dy;
                        if state.drag_active || dist_sq > DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX {
                            state.drag_active = true;
                            let camera = &self.inner.camera;
                            let start_w = camera
                                .screen_to_world(Vec2::new(start.x, start.y), viewport);
                            let cur_w = camera.screen_to_world(Vec2::new(p.x, p.y), viewport);
                            return Some(
                                canvas::Action::publish((self.on_event)(
                                    View2DMessage::DragMoved {
                                        start: start_w,
                                        current: cur_w,
                                    },
                                ))
                                .and_capture(),
                            );
                        }
                    } else {
                        let world = self
                            .inner
                            .camera
                            .screen_to_world(Vec2::new(p.x, p.y), viewport);
                        return Some(
                            canvas::Action::publish((self.on_event)(View2DMessage::HoverAt(world)))
                                .and_capture(),
                        );
                    }
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(p) = cursor_pos {
                    let units = match delta {
                        mouse::ScrollDelta::Lines { y, .. } => *y,
                        mouse::ScrollDelta::Pixels { y, .. } => *y / 50.0,
                    };
                    let factor = (1.15_f32).powf(units);
                    let pivot = Vec2::new(p.x, p.y);
                    return Some(
                        canvas::Action::publish((self.on_event)(View2DMessage::ZoomAt {
                            pivot,
                            factor,
                            viewport,
                        }))
                        .and_capture(),
                    );
                }
            }
            _ => {}
        }
        None
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let viewport = Vec2::new(bounds.width, bounds.height);
        let mode = self.inner.edit_mode;
        let geometry = self.inner.cache.draw(renderer, bounds.size(), |frame| {
            draw_background(frame, bounds);
            draw_grid(frame, &self.inner.camera, viewport);
            let fill_alpha = if mode == EditMode::Sectors { 1.0 } else { 0.55 };
            if self.inner.fills.is_empty() {
                draw_sector_fills_solid(
                    frame,
                    &self.inner.meshes,
                    &self.inner.camera,
                    viewport,
                    fill_alpha,
                );
            } else {
                draw_sector_fills_textured(
                    frame,
                    &self.inner.fills,
                    &self.inner.camera,
                    viewport,
                    fill_alpha,
                );
            }
            draw_sector_highlights(
                frame,
                &self.inner.meshes,
                &self.inner.camera,
                viewport,
                self.inner.hover,
                &self.inner.selection,
            );
            draw_linedefs(
                frame,
                &self.inner.map,
                &self.inner.camera,
                viewport,
                self.inner.hover,
                &self.inner.selection,
                mode,
            );
            // Vertices are only emphasised in Vertices mode; skip the per-vertex
            // dot pass otherwise (selected/hovered vertices still render).
            if mode == EditMode::Vertices {
                draw_vertices(
                    frame,
                    &self.inner.map,
                    &self.inner.camera,
                    viewport,
                    self.inner.hover,
                    &self.inner.selection,
                    1.0,
                );
            } else {
                draw_vertices_only_highlights(
                    frame,
                    &self.inner.map,
                    &self.inner.camera,
                    viewport,
                    self.inner.hover,
                    &self.inner.selection,
                );
            }
            let things_alpha = if mode == EditMode::Things { 1.0 } else { 0.55 };
            draw_things(
                frame,
                &self.inner.map,
                &self.inner.camera,
                viewport,
                &self.inner.config,
                self.inner.hover,
                &self.inner.selection,
                things_alpha,
            );
            if let Some((start, end)) = self.inner.drag_rect {
                draw_drag_rect(frame, &self.inner.camera, viewport, start, end);
            }
        });
        vec![geometry]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.position_in(bounds).is_none() {
            return mouse::Interaction::default();
        }
        if state.panning {
            mouse::Interaction::Grabbing
        } else {
            mouse::Interaction::Crosshair
        }
    }
}

fn draw_background(frame: &mut Frame, bounds: Rectangle) {
    frame.fill_rectangle(
        Point::ORIGIN,
        Size::new(bounds.width, bounds.height),
        Color::from_rgb(0.07, 0.07, 0.09),
    );
}

fn draw_grid(frame: &mut Frame, camera: &Camera2D, viewport: Vec2) {
    let major_world = grid_step(camera.zoom);
    let minor_world = major_world * 0.25;

    let world_min = camera.screen_to_world(Vec2::new(0.0, viewport.y), viewport);
    let world_max = camera.screen_to_world(Vec2::new(viewport.x, 0.0), viewport);

    draw_grid_lines(
        frame,
        camera,
        viewport,
        world_min,
        world_max,
        minor_world,
        Color::from_rgba(0.20, 0.20, 0.22, 0.5),
        0.5,
    );
    draw_grid_lines(
        frame,
        camera,
        viewport,
        world_min,
        world_max,
        major_world,
        Color::from_rgba(0.30, 0.30, 0.34, 1.0),
        0.7,
    );

    let origin_screen = camera.world_to_screen(Vec2::ZERO, viewport);
    let axis_color = Color::from_rgba(0.45, 0.45, 0.50, 1.0);
    let h = Path::line(
        Point::new(0.0, origin_screen.y),
        Point::new(viewport.x, origin_screen.y),
    );
    let v = Path::line(
        Point::new(origin_screen.x, 0.0),
        Point::new(origin_screen.x, viewport.y),
    );
    frame.stroke(&h, Stroke::default().with_color(axis_color).with_width(1.0));
    frame.stroke(&v, Stroke::default().with_color(axis_color).with_width(1.0));
}

fn draw_grid_lines(
    frame: &mut Frame,
    camera: &Camera2D,
    viewport: Vec2,
    world_min: Vec2,
    world_max: Vec2,
    step: f32,
    color: Color,
    width: f32,
) {
    if step <= 0.0 {
        return;
    }
    let stroke = Stroke::default().with_color(color).with_width(width);
    let x_start = (world_min.x / step).floor() * step;
    let mut x = x_start;
    while x <= world_max.x {
        let s = camera.world_to_screen(Vec2::new(x, 0.0), viewport);
        let path = Path::line(Point::new(s.x, 0.0), Point::new(s.x, viewport.y));
        frame.stroke(&path, stroke);
        x += step;
    }
    let y_start = (world_min.y / step).floor() * step;
    let mut y = y_start;
    while y <= world_max.y {
        let s = camera.world_to_screen(Vec2::new(0.0, y), viewport);
        let path = Path::line(Point::new(0.0, s.y), Point::new(viewport.x, s.y));
        frame.stroke(&path, stroke);
        y += step;
    }
}

fn grid_step(zoom: f32) -> f32 {
    let target_pixels = 64.0_f32;
    let world_per_pixel = 1.0 / zoom.max(1e-6);
    let raw = target_pixels * world_per_pixel;
    let exp = raw.log2().round();
    2.0_f32.powf(exp).max(1.0)
}

fn draw_sector_fills_textured(
    frame: &mut Frame,
    fills: &[FillTile],
    camera: &Camera2D,
    viewport: Vec2,
    alpha: f32,
) {
    for tile in fills {
        // World max-y maps to screen top because the camera flips Y.
        let tl = camera.world_to_screen(
            Vec2::new(tile.world_min.x, tile.world_max.y),
            viewport,
        );
        let br = camera.world_to_screen(
            Vec2::new(tile.world_max.x, tile.world_min.y),
            viewport,
        );
        let w = (br.x - tl.x).max(0.0);
        let h = (br.y - tl.y).max(0.0);
        if w == 0.0 || h == 0.0 {
            continue;
        }
        // Skip tiles that are completely outside the viewport bounds.
        if br.x < 0.0 || br.y < 0.0 || tl.x > viewport.x || tl.y > viewport.y {
            continue;
        }
        let img = canvas::Image::new(tile.handle.clone()).opacity(alpha);
        frame.draw_image(Rectangle::new(Point::new(tl.x, tl.y), Size::new(w, h)), img);
    }
}

fn draw_sector_fills_solid(
    frame: &mut Frame,
    meshes: &[(SectorId, FloorMesh)],
    camera: &Camera2D,
    viewport: Vec2,
    alpha: f32,
) {
    let fill = Color::from_rgba(0.15, 0.20, 0.30, 0.6 * alpha);
    for (_, mesh) in meshes {
        let mut tri_path = canvas::path::Builder::new();
        let mut i = 0;
        while i + 2 < mesh.indices.len() {
            let a = mesh.positions[mesh.indices[i] as usize];
            let b = mesh.positions[mesh.indices[i + 1] as usize];
            let c = mesh.positions[mesh.indices[i + 2] as usize];
            let pa = camera.world_to_screen(Vec2::new(a[0], a[1]), viewport);
            let pb = camera.world_to_screen(Vec2::new(b[0], b[1]), viewport);
            let pc = camera.world_to_screen(Vec2::new(c[0], c[1]), viewport);
            tri_path.move_to(Point::new(pa.x, pa.y));
            tri_path.line_to(Point::new(pb.x, pb.y));
            tri_path.line_to(Point::new(pc.x, pc.y));
            tri_path.close();
            i += 3;
        }
        frame.fill(&tri_path.build(), fill);
    }
}

fn draw_sector_highlights(
    frame: &mut Frame,
    meshes: &[(SectorId, FloorMesh)],
    camera: &Camera2D,
    viewport: Vec2,
    hover: Option<HighlightKind>,
    selection: &HashSet<HighlightKind>,
) {
    let hover_sector = match hover {
        Some(HighlightKind::Sector(s)) => Some(s),
        _ => None,
    };
    let selected = Color::from_rgba(1.0, 0.3, 0.3, 0.35);
    let hovered = Color::from_rgba(1.0, 0.78, 0.2, 0.28);
    for (sid, mesh) in meshes {
        let is_selected = selection.contains(&HighlightKind::Sector(*sid));
        let is_hover = hover_sector == Some(*sid);
        if !is_selected && !is_hover {
            continue;
        }
        let color = if is_selected { selected } else { hovered };
        let mut path = canvas::path::Builder::new();
        let mut i = 0;
        while i + 2 < mesh.indices.len() {
            let a = mesh.positions[mesh.indices[i] as usize];
            let b = mesh.positions[mesh.indices[i + 1] as usize];
            let c = mesh.positions[mesh.indices[i + 2] as usize];
            let pa = camera.world_to_screen(Vec2::new(a[0], a[1]), viewport);
            let pb = camera.world_to_screen(Vec2::new(b[0], b[1]), viewport);
            let pc = camera.world_to_screen(Vec2::new(c[0], c[1]), viewport);
            path.move_to(Point::new(pa.x, pa.y));
            path.line_to(Point::new(pb.x, pb.y));
            path.line_to(Point::new(pc.x, pc.y));
            path.close();
            i += 3;
        }
        frame.fill(&path.build(), color);
    }
}

fn draw_linedefs(
    frame: &mut Frame,
    map: &Map,
    camera: &Camera2D,
    viewport: Vec2,
    hover: Option<HighlightKind>,
    selection: &HashSet<HighlightKind>,
    mode: EditMode,
) {
    let active_alpha = if mode == EditMode::Linedefs || mode == EditMode::Sectors {
        1.0
    } else {
        0.55
    };
    let one_sided = Stroke::default()
        .with_color(Color::from_rgba(0.85, 0.85, 0.90, active_alpha))
        .with_width(1.2);
    let two_sided = Stroke::default()
        .with_color(Color::from_rgba(0.55, 0.65, 0.80, 0.9 * active_alpha))
        .with_width(1.0);
    let hovered = Stroke::default()
        .with_color(Color::from_rgb(1.0, 0.7, 0.2))
        .with_width(2.5);
    let selected = Stroke::default()
        .with_color(Color::from_rgb(1.0, 0.3, 0.3))
        .with_width(2.5);

    for (id, line) in &map.linedefs {
        let (Some(v1), Some(v2)) = (map.vertices.get(line.v1), map.vertices.get(line.v2)) else {
            continue;
        };
        let p1 = camera.world_to_screen(Vec2::new(v1.x as f32, v1.y as f32), viewport);
        let p2 = camera.world_to_screen(Vec2::new(v2.x as f32, v2.y as f32), viewport);
        let path = Path::line(Point::new(p1.x, p1.y), Point::new(p2.x, p2.y));

        let is_selected = selection.contains(&HighlightKind::Linedef(id));
        let is_hovered = matches!(hover, Some(HighlightKind::Linedef(h)) if h == id);
        let stroke = if is_selected {
            selected
        } else if is_hovered {
            hovered
        } else if line.left.is_some() && line.right.is_some() {
            two_sided
        } else {
            one_sided
        };
        frame.stroke(&path, stroke);
    }
}

fn draw_vertices_only_highlights(
    frame: &mut Frame,
    map: &Map,
    camera: &Camera2D,
    viewport: Vec2,
    hover: Option<HighlightKind>,
    selection: &HashSet<HighlightKind>,
) {
    let hovered = Color::from_rgb(1.0, 0.55, 0.1);
    let selected = Color::from_rgb(1.0, 0.2, 0.2);
    let radius = (camera.zoom * 0.6).clamp(2.5, 4.0);
    for (id, v) in &map.vertices {
        let is_sel = selection.contains(&HighlightKind::Vertex(id));
        let is_hov = matches!(hover, Some(HighlightKind::Vertex(x)) if x == id);
        if !is_sel && !is_hov {
            continue;
        }
        let s = camera.world_to_screen(Vec2::new(v.x as f32, v.y as f32), viewport);
        let color = if is_sel { selected } else { hovered };
        frame.fill(&Path::circle(Point::new(s.x, s.y), radius), color);
    }
}

fn draw_vertices(
    frame: &mut Frame,
    map: &Map,
    camera: &Camera2D,
    viewport: Vec2,
    hover: Option<HighlightKind>,
    selection: &HashSet<HighlightKind>,
    _alpha: f32,
) {
    let visible = camera.zoom >= 0.15;
    let base = Color::from_rgb(1.0, 0.85, 0.3);
    let hovered = Color::from_rgb(1.0, 0.55, 0.1);
    let selected = Color::from_rgb(1.0, 0.2, 0.2);
    let radius = (camera.zoom * 0.6).clamp(1.5, 3.5);
    let highlight_radius = radius + 2.0;

    for (id, v) in &map.vertices {
        let s = camera.world_to_screen(Vec2::new(v.x as f32, v.y as f32), viewport);
        let is_sel = selection.contains(&HighlightKind::Vertex(id));
        let is_hov = matches!(hover, Some(HighlightKind::Vertex(x)) if x == id);
        if !visible && !is_sel && !is_hov {
            continue;
        }
        let (color, r) = if is_sel {
            (selected, highlight_radius)
        } else if is_hov {
            (hovered, highlight_radius)
        } else {
            (base, radius)
        };
        frame.fill(&Path::circle(Point::new(s.x, s.y), r), color);
    }
}

fn draw_things(
    frame: &mut Frame,
    map: &Map,
    camera: &Camera2D,
    viewport: Vec2,
    config: &GameConfig,
    hover: Option<HighlightKind>,
    selection: &HashSet<HighlightKind>,
    alpha: f32,
) {
    let radius_world = 16.0_f32;
    for (id, t) in &map.things {
        let world_x = t.x as f32;
        let world_y = t.y as f32;
        let center = camera.world_to_screen(Vec2::new(world_x, world_y), viewport);
        let radius_px = (radius_world * camera.zoom).clamp(3.0, 18.0);

        let rad = (t.angle as f32).to_radians();
        let dx = rad.cos();
        let dy = rad.sin();
        let arrow_world = Vec2::new(world_x + dx * radius_world, world_y + dy * radius_world);
        let arrow_end = camera.world_to_screen(arrow_world, viewport);

        let base = thing_color(config, t.kind);
        let base = with_alpha(base, alpha);
        let is_selected = selection.contains(&HighlightKind::Thing(id));
        let is_hover = matches!(hover, Some(HighlightKind::Thing(h)) if h == id);
        // Highlights stay full-opacity even when the things layer is dimmed.
        let (fill, ring_color, ring_width) = if is_selected {
            (with_alpha(base, 1.0), Color::from_rgb(1.0, 0.25, 0.25), 2.5)
        } else if is_hover {
            (with_alpha(base, 1.0), Color::from_rgb(1.0, 0.75, 0.2), 2.0)
        } else {
            (base, Color::from_rgba(0.0, 0.0, 0.0, 0.85 * alpha), 1.0)
        };

        frame.fill(&Path::circle(Point::new(center.x, center.y), radius_px), fill);
        let ring = Path::circle(Point::new(center.x, center.y), radius_px);
        frame.stroke(
            &ring,
            Stroke::default().with_color(ring_color).with_width(ring_width),
        );
        let arrow = Path::line(
            Point::new(center.x, center.y),
            Point::new(arrow_end.x, arrow_end.y),
        );
        frame.stroke(
            &arrow,
            Stroke::default()
                .with_color(Color::from_rgba(0.0, 0.0, 0.0, 0.95 * alpha))
                .with_width(2.0),
        );
    }
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color::from_rgba(c.r, c.g, c.b, c.a * a)
}

fn thing_color(config: &GameConfig, kind: u16) -> Color {
    let category = config
        .thing_type(kind)
        .map(|t| t.category.to_ascii_lowercase())
        .unwrap_or_default();
    match category.as_str() {
        "players" | "playerstart" | "starts" => Color::from_rgb(0.4, 1.0, 0.4),
        "monsters" => Color::from_rgb(1.0, 0.35, 0.35),
        "weapons" => Color::from_rgb(1.0, 0.85, 0.2),
        "ammunition" | "ammo" => Color::from_rgb(1.0, 0.95, 0.6),
        "powerups" => Color::from_rgb(1.0, 0.4, 1.0),
        "health" => Color::from_rgb(0.5, 1.0, 1.0),
        "armor" => Color::from_rgb(0.3, 0.6, 1.0),
        "keys" => Color::from_rgb(1.0, 0.9, 0.0),
        "obstacles" | "decoration" | "lights" => Color::from_rgb(0.7, 0.7, 0.7),
        "teleports" => Color::from_rgb(0.4, 1.0, 0.85),
        _ => Color::from_rgb(0.85, 0.85, 0.85),
    }
}

fn draw_drag_rect(frame: &mut Frame, camera: &Camera2D, viewport: Vec2, start: Vec2, end: Vec2) {
    let s = camera.world_to_screen(start, viewport);
    let e = camera.world_to_screen(end, viewport);
    let min_x = s.x.min(e.x);
    let max_x = s.x.max(e.x);
    let min_y = s.y.min(e.y);
    let max_y = s.y.max(e.y);
    let fill = Color::from_rgba(0.4, 0.7, 1.0, 0.15);
    frame.fill_rectangle(
        Point::new(min_x, min_y),
        Size::new(max_x - min_x, max_y - min_y),
        fill,
    );
    let path = Path::new(|p| {
        p.move_to(Point::new(min_x, min_y));
        p.line_to(Point::new(max_x, min_y));
        p.line_to(Point::new(max_x, max_y));
        p.line_to(Point::new(min_x, max_y));
        p.close();
    });
    let stroke = Stroke::default()
        .with_color(Color::from_rgba(0.5, 0.8, 1.0, 0.9))
        .with_width(1.0);
    frame.stroke(&path, stroke);
}

pub fn map_aabb(map: &Map) -> Option<(Vec2, Vec2)> {
    let mut iter = map.vertices.iter().map(|(_, v)| Vec2::new(v.x as f32, v.y as f32));
    let first = iter.next()?;
    let mut min = first;
    let mut max = first;
    for p in iter {
        min = min.min(p);
        max = max.max(p);
    }
    Some((min, max))
}
