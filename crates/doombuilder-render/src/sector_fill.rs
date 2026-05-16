// ABOUTME: CPU-rasterise each sector's polygon into an RGBA image with its
// ABOUTME: flat texture tiled inside. Used by the 2D viewport to render the
// ABOUTME: UDB-style "show textures" overlay; one image per sector.

use doombuilder_core::map::{Map, SectorId};
use doombuilder_core::textures::{TextureImage, TextureSet};

use crate::mesh::FloorMesh;

/// Pixel image of a sector's filled polygon, 1 px per world unit. Origin
/// records the world-space top-left so the GUI can place the image at the
/// right screen position.
#[derive(Debug, Clone)]
pub struct SectorFill {
    pub width: u32,
    pub height: u32,
    /// World-space top-left corner: (min_x, max_y).
    pub origin_world: (f32, f32),
    pub rgba: Vec<u8>,
}

/// Which sector flat to rasterise into the 2D fill image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillSlot {
    Floor,
    Ceiling,
}

pub fn rasterise_sector_fill(
    map: &Map,
    sector: SectorId,
    mesh: &FloorMesh,
    textures: &TextureSet,
) -> Option<SectorFill> {
    rasterise_sector_fill_slot(map, sector, mesh, textures, FillSlot::Floor)
}

pub fn rasterise_sector_fill_slot(
    map: &Map,
    sector: SectorId,
    mesh: &FloorMesh,
    textures: &TextureSet,
    slot: FillSlot,
) -> Option<SectorFill> {
    if mesh.indices.is_empty() {
        return None;
    }
    let sec = map.sectors.get(sector)?;
    let name = match slot {
        FillSlot::Floor => sec.floor_texture.as_str().to_ascii_uppercase(),
        FillSlot::Ceiling => sec.ceiling_texture.as_str().to_ascii_uppercase(),
    };
    let flat = textures.flats.get(&name)?;

    let (min_x, min_y, max_x, max_y) = aabb(&mesh.positions);
    let x0 = min_x.floor() as i32;
    let y0 = min_y.floor() as i32;
    let x1 = max_x.ceil() as i32;
    let y1 = max_y.ceil() as i32;
    let width = (x1 - x0).max(1) as u32;
    let height = (y1 - y0).max(1) as u32;

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    let mut target = RasterTarget {
        rgba: &mut rgba[..],
        width,
        height,
        origin_x: x0,
        origin_y_top: y1,
    };
    let mut i = 0;
    while i + 2 < mesh.indices.len() {
        let a = mesh.positions[mesh.indices[i] as usize];
        let b = mesh.positions[mesh.indices[i + 1] as usize];
        let c = mesh.positions[mesh.indices[i + 2] as usize];
        rasterise_triangle(&mut target, a, b, c, flat);
        i += 3;
    }

    Some(SectorFill {
        width,
        height,
        origin_world: (x0 as f32, y1 as f32),
        rgba,
    })
}

/// Rasterise a sector's polygon filled with a solid RGBA color. Used for
/// the "brightness levels" view where the fill encodes sector light.
pub fn rasterise_sector_solid(
    sector: SectorId,
    mesh: &FloorMesh,
    color: [u8; 4],
) -> Option<SectorFill> {
    let _ = sector;
    if mesh.indices.is_empty() {
        return None;
    }
    let (min_x, min_y, max_x, max_y) = aabb(&mesh.positions);
    let x0 = min_x.floor() as i32;
    let y0 = min_y.floor() as i32;
    let x1 = max_x.ceil() as i32;
    let y1 = max_y.ceil() as i32;
    let _ = y0;
    let width = (x1 - x0).max(1) as u32;
    let height = (y1 - y0).max(1) as u32;
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    let mut target = RasterTarget {
        rgba: &mut rgba[..],
        width,
        height,
        origin_x: x0,
        origin_y_top: y1,
    };
    let mut i = 0;
    while i + 2 < mesh.indices.len() {
        let a = mesh.positions[mesh.indices[i] as usize];
        let b = mesh.positions[mesh.indices[i + 1] as usize];
        let c = mesh.positions[mesh.indices[i + 2] as usize];
        rasterise_triangle_solid(&mut target, a, b, c, color);
        i += 3;
    }
    Some(SectorFill {
        width,
        height,
        origin_world: (x0 as f32, y1 as f32),
        rgba,
    })
}

/// Mutable raster surface: the RGBA buffer plus its dimensions and the
/// world-space coordinates of its top-left pixel.
struct RasterTarget<'a> {
    rgba: &'a mut [u8],
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y_top: i32,
}

fn rasterise_triangle_solid(
    target: &mut RasterTarget,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: [u8; 4],
) {
    let width = target.width;
    let height = target.height;
    let origin_x = target.origin_x;
    let origin_y_top = target.origin_y_top;
    let rgba = &mut *target.rgba;
    let min_x = a[0].min(b[0]).min(c[0]).floor() as i32;
    let max_x = a[0].max(b[0]).max(c[0]).ceil() as i32;
    let min_y = a[1].min(b[1]).min(c[1]).floor() as i32;
    let max_y = a[1].max(b[1]).max(c[1]).ceil() as i32;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            // Half-plane test using sign-of-cross-product against each edge.
            let s_ab = edge_solid(a, b, p);
            let s_bc = edge_solid(b, c, p);
            let s_ca = edge_solid(c, a, p);
            let inside = (s_ab >= 0.0 && s_bc >= 0.0 && s_ca >= 0.0)
                || (s_ab <= 0.0 && s_bc <= 0.0 && s_ca <= 0.0);
            if !inside {
                continue;
            }
            // World y goes up; image y goes down → flip to image row.
            let img_x = x - origin_x;
            let img_y = origin_y_top - 1 - y;
            if img_x < 0 || img_y < 0 || img_x as u32 >= width || img_y as u32 >= height {
                continue;
            }
            let idx = ((img_y as u32 * width + img_x as u32) * 4) as usize;
            rgba[idx..idx + 4].copy_from_slice(&color);
        }
    }
}

fn edge_solid(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

fn aabb(positions: &[[f32; 2]]) -> (f32, f32, f32, f32) {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for p in positions {
        if p[0] < min_x {
            min_x = p[0];
        }
        if p[1] < min_y {
            min_y = p[1];
        }
        if p[0] > max_x {
            max_x = p[0];
        }
        if p[1] > max_y {
            max_y = p[1];
        }
    }
    (min_x, min_y, max_x, max_y)
}

fn rasterise_triangle(
    target: &mut RasterTarget,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    flat: &TextureImage,
) {
    let width = target.width;
    let height = target.height;
    let origin_x = target.origin_x;
    let origin_y_top = target.origin_y_top;
    let rgba = &mut *target.rgba;
    let area = edge(a, b, c);
    if area.abs() < 1e-6 {
        return;
    }
    let sign = if area > 0.0 { 1.0 } else { -1.0 };

    let tri_min_x = a[0].min(b[0]).min(c[0]).floor() as i32;
    let tri_max_x = a[0].max(b[0]).max(c[0]).ceil() as i32;
    let tri_min_y = a[1].min(b[1]).min(c[1]).floor() as i32;
    let tri_max_y = a[1].max(b[1]).max(c[1]).ceil() as i32;

    let img_w = width as i32;
    let img_h = height as i32;

    for wy in tri_min_y..tri_max_y {
        for wx in tri_min_x..tri_max_x {
            let p = [wx as f32 + 0.5, wy as f32 + 0.5];
            let w0 = edge(b, c, p) * sign;
            let w1 = edge(c, a, p) * sign;
            let w2 = edge(a, b, p) * sign;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let img_x = wx - origin_x;
            // World Y goes up, image row 0 is at world max_y, so flip.
            let img_y = (origin_y_top - 1) - wy;
            if img_x < 0 || img_x >= img_w || img_y < 0 || img_y >= img_h {
                continue;
            }
            let fx = wx.rem_euclid(64) as usize;
            let fy = (63 - wy.rem_euclid(64)) as usize;
            let src = (fy * 64 + fx) * 4;
            let dst = ((img_y as usize) * (width as usize) + img_x as usize) * 4;
            if src + 3 < flat.rgba.len() {
                rgba[dst] = flat.rgba[src];
                rgba[dst + 1] = flat.rgba[src + 1];
                rgba[dst + 2] = flat.rgba[src + 2];
                rgba[dst + 3] = flat.rgba[src + 3];
            }
        }
    }
}

fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (p[0] - a[0]) * (b[1] - a[1]) - (p[1] - a[1]) * (b[0] - a[0])
}
