// ABOUTME: 3D viewport built on iced's wgpu shader widget. Renders textured
// ABOUTME: floors, ceilings, and walls with sector-light shading. Orbit camera.
// ABOUTME: Geometry is built CPU-side per frame; GPU textures are cached lazily.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use doombuilder_core::config::GameConfig;
use doombuilder_core::map::{Map, SectorId};
use doombuilder_core::textures::{TextureImage, TextureSet};
use doombuilder_render::{FloorMesh, SpatialIndex, Wall, WallKind};
use glam::{Mat4, Vec3};
use iced::mouse;
use iced::widget::shader::{self, Action, Pipeline as PipelineTrait, Primitive};
use iced::{Element, Length, Rectangle};
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) light: f32,
    @location(3) tint: vec3<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) light: f32,
    @location(2) tint: vec3<f32>,
};

@vertex
fn vs(in: VIn) -> VOut {
    var o: VOut;
    o.clip = u.view_proj * vec4<f32>(in.pos, 1.0);
    o.uv = in.uv;
    o.light = in.light;
    o.tint = in.tint;
    return o;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, in.uv);
    // Editor lighting (not vanilla Doom playback): high ambient floor +
    // flat gamma so even light=0 sectors are clearly readable. Engines
    // apply far harsher light tables; here legibility wins.
    let lit = 0.72 + 0.28 * pow(in.light, 0.4);
    return vec4<f32>(c.rgb * in.tint * lit, 1.0);
}
"#;

const MISSING_TEXTURE_KEY: &str = "__missing__";
const SOLID_TEXTURE_KEY: &str = "__solid__";
const SKY_TEXTURE_KEY: &str = "__sky__";
const SKY_FLAT_NAME: &str = "F_SKY1";

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex3D {
    pos: [f32; 3],
    uv: [f32; 2],
    light: f32,
    tint: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct DrawBatch {
    pub texture_name: String,
    pub start: u32,
    pub count: u32,
}

#[derive(Debug, Clone, Default)]
pub struct View3DGeometry {
    pub vertices: Vec<Vertex3D>,
    pub batches: Vec<DrawBatch>,
}

pub fn build_geometry(
    map: &Map,
    sector_meshes: &[(SectorId, FloorMesh)],
    walls: &[Wall],
    textures: &TextureSet,
    spatial: Option<&SpatialIndex>,
    config: &GameConfig,
    full_brightness: bool,
) -> View3DGeometry {
    let mut by_texture: HashMap<String, Vec<Vertex3D>> = HashMap::new();

    for (sid, mesh) in sector_meshes {
        let Some(sector) = map.sectors.get(*sid) else {
            continue;
        };
        let light = if full_brightness {
            1.0
        } else {
            (sector.light as f32 / 255.0).clamp(0.0, 1.0)
        };
        let floor_y = sector.floor_height as f32;
        let ceil_y = sector.ceiling_height as f32;
        let floor_name = sector.floor_texture.as_str().to_ascii_uppercase();
        let ceil_name = sector.ceiling_texture.as_str().to_ascii_uppercase();
        // F_SKY1 surfaces render via a dedicated sky-blue bucket at full
        // brightness instead of being dropped (which left a "hole in the
        // world" you could see the chrome through).
        let floor_tex = if floor_name == SKY_FLAT_NAME {
            SKY_TEXTURE_KEY.to_string()
        } else {
            key(&floor_name)
        };
        let ceil_tex = if ceil_name == SKY_FLAT_NAME {
            SKY_TEXTURE_KEY.to_string()
        } else {
            key(&ceil_name)
        };
        let floor_light = if floor_name == SKY_FLAT_NAME {
            1.0
        } else {
            light
        };
        let ceil_light = if ceil_name == SKY_FLAT_NAME {
            1.0
        } else {
            light
        };

        let mut i = 0;
        while i + 2 < mesh.indices.len() {
            let a = mesh.positions[mesh.indices[i] as usize];
            let b = mesh.positions[mesh.indices[i + 1] as usize];
            let c = mesh.positions[mesh.indices[i + 2] as usize];

            push_floor_tri(
                by_texture.entry(floor_tex.clone()).or_default(),
                a,
                c,
                b,
                floor_y,
                floor_light,
            );
            push_floor_tri(
                by_texture.entry(ceil_tex.clone()).or_default(),
                a,
                b,
                c,
                ceil_y,
                ceil_light,
            );
            i += 3;
        }
    }

    for wall in walls {
        let Some(sec) = map.sectors.get(wall.facing_sector) else {
            continue;
        };
        let light = if full_brightness {
            1.0
        } else {
            (sec.light as f32 / 255.0).clamp(0.0, 1.0)
        };
        let tex_name = wall_texture_name(map, wall);
        let tex_key = key(tex_name.as_str());
        let (tex_w, tex_h) = textures
            .texture_or_flat(tex_name.as_str())
            .map(|t| (t.width.max(1) as f32, t.height.max(1) as f32))
            .unwrap_or((64.0, 64.0));
        let tex_w = if tex_w == 0.0 { 64.0 } else { tex_w };
        let tex_h = if tex_h == 0.0 { 64.0 } else { tex_h };
        push_wall(
            by_texture.entry(tex_key).or_default(),
            wall,
            light,
            tex_w,
            tex_h,
        );
    }

    // Things: render as colored boxes, color from game config category.
    if let Some(spatial) = spatial {
        let solid_bucket = by_texture.entry(SOLID_TEXTURE_KEY.to_string()).or_default();
        for (_, thing) in &map.things {
            let tx = thing.x as f32;
            let ty = thing.y as f32;
            let floor_y = match spatial.sector_at(tx, ty) {
                Some(sid) => map
                    .sectors
                    .get(sid)
                    .map(|s| s.floor_height as f32)
                    .unwrap_or(0.0),
                None => 0.0,
            };
            let color = thing_color(config, thing.kind);
            push_thing_box(solid_bucket, tx, ty, floor_y, color);
        }
    }

    // Flatten into one vertex buffer with batch start/count records.
    let mut vertices: Vec<Vertex3D> = Vec::new();
    let mut batches: Vec<DrawBatch> = Vec::new();
    let mut keys: Vec<String> = by_texture.keys().cloned().collect();
    keys.sort();
    for k in keys {
        let verts = by_texture.remove(&k).unwrap();
        if verts.is_empty() {
            continue;
        }
        let start = vertices.len() as u32;
        let count = verts.len() as u32;
        vertices.extend(verts);
        batches.push(DrawBatch {
            texture_name: k,
            start,
            count,
        });
    }

    View3DGeometry { vertices, batches }
}

fn key(name: &str) -> String {
    let trimmed = name.trim_end_matches('\0');
    if trimmed.is_empty() || trimmed == "-" {
        MISSING_TEXTURE_KEY.to_string()
    } else {
        trimmed.to_ascii_uppercase()
    }
}

fn wall_texture_name(map: &Map, wall: &Wall) -> doombuilder_core::map::TextureName {
    let Some(side) = map.sidedefs.get(wall.sidedef) else {
        return doombuilder_core::map::TextureName([0; 8]);
    };
    match wall.kind {
        WallKind::Solid => side.middle_texture,
        WallKind::Upper => side.upper_texture,
        WallKind::Lower => side.lower_texture,
    }
}

fn push_floor_tri(
    out: &mut Vec<Vertex3D>,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    y: f32,
    light: f32,
) {
    out.push(world_floor_vertex(a, y, light));
    out.push(world_floor_vertex(b, y, light));
    out.push(world_floor_vertex(c, y, light));
}

fn world_floor_vertex(p: [f32; 2], y: f32, light: f32) -> Vertex3D {
    Vertex3D {
        pos: [p[0], y, -p[1]],
        uv: [p[0] / 64.0, p[1] / 64.0],
        light,
        tint: [1.0, 1.0, 1.0],
    }
}

fn push_wall(out: &mut Vec<Vertex3D>, wall: &Wall, light: f32, tex_w: f32, tex_h: f32) {
    let q = wall.quad; // [bot_left, bot_right, top_right, top_left] in (x, y, z=height).
    let length = {
        let dx = q[1][0] - q[0][0];
        let dy = q[1][1] - q[0][1];
        (dx * dx + dy * dy).sqrt()
    };
    let height = q[2][2] - q[0][2];
    let u_max = (length / tex_w).max(1e-6);
    let v_max = (height / tex_h).max(1e-6);
    let bl = wall_vertex(q[0], 0.0, v_max, light);
    let br = wall_vertex(q[1], u_max, v_max, light);
    let tr = wall_vertex(q[2], u_max, 0.0, light);
    let tl = wall_vertex(q[3], 0.0, 0.0, light);
    out.push(bl);
    out.push(tr);
    out.push(br);
    out.push(bl);
    out.push(tl);
    out.push(tr);
}

fn wall_vertex(p: [f32; 3], u: f32, v: f32, light: f32) -> Vertex3D {
    Vertex3D {
        pos: [p[0], p[2], -p[1]],
        uv: [u, v],
        light,
        tint: [1.0, 1.0, 1.0],
    }
}

fn push_thing_box(
    out: &mut Vec<Vertex3D>,
    doom_x: f32,
    doom_y: f32,
    floor_y: f32,
    color: [f32; 3],
) {
    let r = 16.0_f32;
    let h = 56.0_f32;
    let cx = doom_x;
    let cz = -doom_y;
    let y0 = floor_y;
    let y1 = floor_y + h;
    // 8 box corners.
    let p = [
        [cx - r, y0, cz - r], // 0 nbl
        [cx + r, y0, cz - r], // 1 nbr
        [cx + r, y0, cz + r], // 2 fbr
        [cx - r, y0, cz + r], // 3 fbl
        [cx - r, y1, cz - r], // 4 ntl
        [cx + r, y1, cz - r], // 5 ntr
        [cx + r, y1, cz + r], // 6 ftr
        [cx - r, y1, cz + r], // 7 ftl
    ];
    let mut push = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
        for v in [a, b, c] {
            out.push(Vertex3D {
                pos: v,
                uv: [0.0, 0.0],
                light: 1.0,
                tint: color,
            });
        }
    };
    // Bottom (skipped — sits on the floor).
    // Top (CCW from above).
    push(p[4], p[5], p[6]);
    push(p[4], p[6], p[7]);
    // North face (z = -r), CCW when looking from +Z.
    push(p[0], p[5], p[1]);
    push(p[0], p[4], p[5]);
    // South face (z = +r).
    push(p[3], p[2], p[6]);
    push(p[3], p[6], p[7]);
    // East face (x = +r).
    push(p[1], p[6], p[2]);
    push(p[1], p[5], p[6]);
    // West face (x = -r).
    push(p[0], p[3], p[7]);
    push(p[0], p[7], p[4]);
}

fn thing_color(config: &GameConfig, kind: u16) -> [f32; 3] {
    let category = config
        .thing_type(kind)
        .map(|t| t.category.to_ascii_lowercase())
        .unwrap_or_default();
    match category.as_str() {
        "players" | "playerstart" | "starts" => [0.4, 1.0, 0.4],
        "monsters" => [1.0, 0.35, 0.35],
        "weapons" => [1.0, 0.85, 0.2],
        "ammunition" | "ammo" => [1.0, 0.95, 0.6],
        "powerups" => [1.0, 0.4, 1.0],
        "health" => [0.5, 1.0, 1.0],
        "armor" => [0.3, 0.6, 1.0],
        "keys" => [1.0, 0.9, 0.0],
        "obstacles" | "decoration" | "lights" => [0.7, 0.7, 0.7],
        "teleports" => [0.4, 1.0, 0.85],
        _ => [0.85, 0.85, 0.85],
    }
}

// ---- Camera ---------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Camera3D {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for Camera3D {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.5,
            distance: 1500.0,
        }
    }
}

impl Camera3D {
    pub fn frame_aabb(&mut self, min: Vec3, max: Vec3) {
        self.target = (min + max) * 0.5;
        let extent = (max - min).max(Vec3::splat(64.0));
        self.distance = extent.length() * 0.9;
        self.yaw = 0.0;
        self.pitch = 0.6;
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let position = self.target
            + Vec3::new(
                self.distance * self.pitch.cos() * self.yaw.sin(),
                self.distance * self.pitch.sin().max(0.05),
                self.distance * self.pitch.cos() * self.yaw.cos(),
            );
        let view = Mat4::look_at_rh(position, self.target, Vec3::Y);
        let proj = Mat4::perspective_rh(60.0_f32.to_radians(), aspect.max(0.01), 1.0, 100_000.0);
        proj * view
    }
}

// ---- Iced Program / Primitive --------------------------------------------

#[derive(Debug, Clone)]
pub enum View3DMessage {
    OrbitBy { dx: f32, dy: f32 },
    Zoom { factor: f32 },
}

pub struct View3D {
    pub geometry: Arc<View3DGeometry>,
    pub textures: Arc<TextureSet>,
    pub camera: Camera3D,
}

impl View3D {
    pub fn into_widget<Message: 'static + Clone>(
        self,
        on_event: impl Fn(View3DMessage) -> Message + 'static,
    ) -> Element<'static, Message> {
        shader::Shader::new(View3DProgram {
            inner: self,
            on_event: Box::new(on_event),
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

struct View3DProgram<Message> {
    inner: View3D,
    on_event: Box<dyn Fn(View3DMessage) -> Message>,
}

#[derive(Default)]
pub struct InternalState {
    dragging: bool,
    last_cursor: Option<iced::Point>,
}

impl<Message: 'static> shader::Program<Message> for View3DProgram<Message> {
    type State = InternalState;
    type Primitive = Map3DPrimitive;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.position_in(bounds).is_some() =>
            {
                state.dragging = true;
                state.last_cursor = cursor.position_in(bounds);
                return Some(Action::capture());
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.dragging = false;
                state.last_cursor = None;
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                if let Some(pos) = cursor.position_in(bounds) {
                    if let Some(prev) = state.last_cursor.replace(pos) {
                        let dx = pos.x - prev.x;
                        let dy = pos.y - prev.y;
                        return Some(
                            Action::publish((self.on_event)(View3DMessage::OrbitBy { dx, dy }))
                                .and_capture(),
                        );
                    }
                }
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let units = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 50.0,
                };
                let factor = (1.15_f32).powf(-units);
                return Some(
                    Action::publish((self.on_event)(View3DMessage::Zoom { factor })).and_capture(),
                );
            }
            _ => {}
        }
        None
    }

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Map3DPrimitive {
        let aspect = (bounds.width / bounds.height.max(1.0)).max(0.01);
        let view_proj = self.inner.camera.view_proj(aspect).to_cols_array_2d();
        Map3DPrimitive {
            geometry: self.inner.geometry.clone(),
            textures: self.inner.textures.clone(),
            view_proj,
        }
    }
}

#[derive(Debug)]
pub struct Map3DPrimitive {
    pub geometry: Arc<View3DGeometry>,
    pub textures: Arc<TextureSet>,
    pub view_proj: [[f32; 4]; 4],
}

impl Primitive for Map3DPrimitive {
    type Pipeline = Map3DPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        // Uniforms.
        queue.write_buffer(
            &pipeline.uniform_buffer,
            0,
            bytemuck::cast_slice(&self.view_proj),
        );

        // Vertex buffer (resize as needed).
        let bytes: &[u8] = bytemuck::cast_slice(&self.geometry.vertices);
        if bytes.len() as u64 > pipeline.vertex_capacity {
            pipeline.vertex_capacity = (bytes.len() as u64).next_power_of_two().max(4096);
            pipeline.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("doombuilder vbuf"),
                size: pipeline.vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            queue.write_buffer(&pipeline.vertex_buffer, 0, bytes);
        }

        // Depth texture must match the color attachment, which is the full
        // surface target rather than the widget's logical bounds.
        let phys = viewport.physical_size();
        let target_w = phys.width.max(1);
        let target_h = phys.height.max(1);
        if pipeline.depth_size != (target_w, target_h) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("doombuilder depth"),
                size: wgpu::Extent3d {
                    width: target_w,
                    height: target_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            pipeline.depth_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            pipeline.depth_texture = tex;
            pipeline.depth_size = (target_w, target_h);
        }

        // Lazily upload any new texture referenced by a batch.
        for batch in &self.geometry.batches {
            if pipeline
                .texture_bind_groups
                .contains_key(&batch.texture_name)
            {
                continue;
            }
            let bg = if batch.texture_name == SOLID_TEXTURE_KEY {
                pipeline.upload_solid_white(device, queue)
            } else if batch.texture_name == SKY_TEXTURE_KEY {
                // Flat sky-blue stand-in for F_SKY1 surfaces. Not a real
                // panorama, but reads as "sky" instead of a checker hole.
                pipeline.upload_solid_color(device, queue, [120, 158, 200, 255])
            } else {
                let img = if batch.texture_name == MISSING_TEXTURE_KEY {
                    None
                } else {
                    self.textures.texture_or_flat(&batch.texture_name)
                };
                pipeline.upload_texture(device, queue, img)
            };
            pipeline
                .texture_bind_groups
                .insert(batch.texture_name.clone(), bg);
        }
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("doombuilder 3d pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    // CRITICAL: LoadOp::Clear here clears the *entire* color
                    // attachment regardless of scissor — that wipes out any
                    // canvas widget rendered before us in the frame. Use
                    // LoadOp::Load so the scissor-confined draw calls render
                    // on top of the underlying widget chrome. The parent
                    // container's background fills the panel area first.
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &pipeline.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // Critical for widgets that aren't at (0, 0) full-window: without
        // an explicit viewport, NDC (-1..1) maps to the entire render
        // target, so geometry projects to the centre of the full window and
        // the scissor clips it all away when the widget is offset (e.g. in
        // a side panel). The viewport must match the widget's pixel rect.
        pass.set_viewport(
            clip_bounds.x as f32,
            clip_bounds.y as f32,
            clip_bounds.width.max(1) as f32,
            clip_bounds.height.max(1) as f32,
            0.0,
            1.0,
        );
        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width.max(1),
            clip_bounds.height.max(1),
        );

        if self.geometry.vertices.is_empty() {
            return;
        }

        pass.set_pipeline(&pipeline.render_pipeline);
        pass.set_bind_group(0, &pipeline.uniform_bind_group, &[]);
        pass.set_vertex_buffer(0, pipeline.vertex_buffer.slice(..));

        for batch in &self.geometry.batches {
            let Some(bg) = pipeline.texture_bind_groups.get(&batch.texture_name) else {
                continue;
            };
            pass.set_bind_group(1, bg, &[]);
            pass.draw(batch.start..(batch.start + batch.count), 0..1);
        }
    }
}

// ---- Pipeline ------------------------------------------------------------

pub struct Map3DPipeline {
    render_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    texture_bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: u64,
    #[allow(dead_code)]
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    depth_size: (u32, u32),
    texture_bind_groups: HashMap<String, wgpu::BindGroup>,
}

impl PipelineTrait for Map3DPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self
    where
        Self: Sized,
    {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("doombuilder 3d shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniform layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let texture_bind_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("texture layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d pipeline layout"),
            bind_group_layouts: &[&uniform_layout, &texture_bind_layout],
            push_constant_ranges: &[],
        });

        let vertex_attrs = wgpu::vertex_attr_array![
            0 => Float32x3, // position
            1 => Float32x2, // uv
            2 => Float32,   // light
            3 => Float32x3, // tint
        ];

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3d pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex3D>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &vertex_attrs,
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniform buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform bg"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("texture sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let vertex_capacity = 1 << 16;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertex buffer"),
            size: vertex_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth init"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            render_pipeline,
            uniform_buffer,
            uniform_bind_group,
            texture_bind_layout,
            sampler,
            vertex_buffer,
            vertex_capacity,
            depth_texture,
            depth_view,
            depth_size: (1, 1),
            texture_bind_groups: HashMap::new(),
        }
    }
}

impl Map3DPipeline {
    fn upload_solid_color(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: [u8; 4],
    ) -> wgpu::BindGroup {
        let pixels = rgba.to_vec();
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("solid color"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &pixels,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("solid color bg"),
            layout: &self.texture_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn upload_solid_white(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::BindGroup {
        let pixels: Vec<u8> = vec![255, 255, 255, 255];
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("solid white"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &pixels,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("solid bg"),
            layout: &self.texture_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn upload_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        img: Option<&TextureImage>,
    ) -> wgpu::BindGroup {
        let (w, h, rgba) = match img {
            Some(t) => (t.width as u32, t.height as u32, t.rgba.clone()),
            None => {
                // 2x2 magenta-and-black checker for missing textures.
                let pixels: Vec<u8> = vec![
                    255, 0, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 0, 255, 255,
                ];
                (2, 2, pixels)
            }
        };
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("doom texture"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &rgba,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("doom texture bg"),
            layout: &self.texture_bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}

// ---- World-space AABB helpers ------------------------------------------

/// Compute world-space (y-up) AABB of all loaded geometry, useful to frame the
/// camera when 3D mode opens.
pub fn world_aabb(map: &Map, sector_meshes: &[(SectorId, FloorMesh)]) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut any = false;
    for (sid, mesh) in sector_meshes {
        let Some(sector) = map.sectors.get(*sid) else {
            continue;
        };
        let floor_y = sector.floor_height as f32;
        let ceil_y = sector.ceiling_height as f32;
        for p in &mesh.positions {
            for &y in &[floor_y, ceil_y] {
                let v = Vec3::new(p[0], y, -p[1]);
                min = min.min(v);
                max = max.max(v);
                any = true;
            }
        }
    }
    if any {
        Some((min, max))
    } else {
        None
    }
}
