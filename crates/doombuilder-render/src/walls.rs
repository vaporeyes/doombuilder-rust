// ABOUTME: Build wall quads for 3D mode. One-sided linedefs become a single
// ABOUTME: full-height wall; two-sided linedefs become up to three step quads
// ABOUTME: (upper, middle/sky, lower) where neighbouring sector heights differ.

use doombuilder_core::map::{Map, MapLinedef, SectorId};

#[derive(Debug, Clone, Copy)]
pub enum WallKind {
    /// Full wall on a one-sided linedef.
    Solid,
    /// Step-up between two sectors with different floor heights.
    Lower,
    /// Step-down between two sectors with different ceiling heights.
    Upper,
}

#[derive(Debug, Clone, Copy)]
pub struct Wall {
    pub kind: WallKind,
    /// Sector this wall faces into (used to pick light/colour).
    pub facing_sector: SectorId,
    /// World-space quad in (x, y, z) order: bottom-left, bottom-right,
    /// top-right, top-left. Ready to upload as two triangles.
    pub quad: [[f32; 3]; 4],
}

pub fn build_walls(map: &Map) -> Vec<Wall> {
    let mut out = Vec::new();
    for (_, line) in &map.linedefs {
        emit_walls_for_linedef(map, line, &mut out);
    }
    out
}

fn emit_walls_for_linedef(map: &Map, line: &MapLinedef, out: &mut Vec<Wall>) {
    let Some(v1) = map.vertices.get(line.v1) else {
        return;
    };
    let Some(v2) = map.vertices.get(line.v2) else {
        return;
    };
    let p1 = (v1.x as f32, v1.y as f32);
    let p2 = (v2.x as f32, v2.y as f32);

    let right_sector = line
        .right
        .and_then(|sid| map.sidedefs.get(sid))
        .map(|s| s.sector);
    let left_sector = line
        .left
        .and_then(|sid| map.sidedefs.get(sid))
        .map(|s| s.sector);

    match (right_sector, left_sector) {
        (Some(rs), None) => {
            if let Some(sec) = map.sectors.get(rs) {
                out.push(Wall {
                    kind: WallKind::Solid,
                    facing_sector: rs,
                    quad: quad(p1, p2, sec.floor_height as f32, sec.ceiling_height as f32),
                });
            }
        }
        (None, Some(ls)) => {
            if let Some(sec) = map.sectors.get(ls) {
                out.push(Wall {
                    kind: WallKind::Solid,
                    facing_sector: ls,
                    quad: quad(p2, p1, sec.floor_height as f32, sec.ceiling_height as f32),
                });
            }
        }
        (Some(rs), Some(ls)) => {
            let (Some(rsec), Some(lsec)) = (map.sectors.get(rs), map.sectors.get(ls)) else {
                return;
            };
            // Lower: step where one sector's floor is higher than the other.
            if rsec.floor_height < lsec.floor_height {
                out.push(Wall {
                    kind: WallKind::Lower,
                    facing_sector: ls,
                    quad: quad(p2, p1, rsec.floor_height as f32, lsec.floor_height as f32),
                });
            } else if lsec.floor_height < rsec.floor_height {
                out.push(Wall {
                    kind: WallKind::Lower,
                    facing_sector: rs,
                    quad: quad(p1, p2, lsec.floor_height as f32, rsec.floor_height as f32),
                });
            }
            // Upper: step where one sector's ceiling is lower than the other.
            if rsec.ceiling_height > lsec.ceiling_height {
                out.push(Wall {
                    kind: WallKind::Upper,
                    facing_sector: ls,
                    quad: quad(p2, p1, lsec.ceiling_height as f32, rsec.ceiling_height as f32),
                });
            } else if lsec.ceiling_height > rsec.ceiling_height {
                out.push(Wall {
                    kind: WallKind::Upper,
                    facing_sector: rs,
                    quad: quad(p1, p2, rsec.ceiling_height as f32, lsec.ceiling_height as f32),
                });
            }
        }
        (None, None) => {}
    }
}

fn quad(p_bot: (f32, f32), p_top: (f32, f32), z_bot: f32, z_top: f32) -> [[f32; 3]; 4] {
    [
        [p_bot.0, p_bot.1, z_bot],
        [p_top.0, p_top.1, z_bot],
        [p_top.0, p_top.1, z_top],
        [p_bot.0, p_bot.1, z_top],
    ]
}
