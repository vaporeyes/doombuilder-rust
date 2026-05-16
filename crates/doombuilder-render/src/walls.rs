// ABOUTME: Build wall quads for 3D mode. One-sided linedefs become a single
// ABOUTME: full-height wall; two-sided linedefs become up to three step quads
// ABOUTME: (upper, middle/sky, lower) where neighbouring sector heights differ.

use doombuilder_core::map::{Map, MapLinedef, SectorId, SidedefId};

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
    /// Sidedef the wall renders the texture of (slot is chosen by `kind`).
    pub sidedef: SidedefId,
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

    let right_side = line.right;
    let left_side = line.left;
    let right_sector = right_side
        .and_then(|sid| map.sidedefs.get(sid))
        .map(|s| s.sector);
    let left_sector = left_side
        .and_then(|sid| map.sidedefs.get(sid))
        .map(|s| s.sector);

    match (right_sector, left_sector) {
        (Some(rs), None) => {
            if let (Some(sec), Some(rside)) = (map.sectors.get(rs), right_side) {
                out.push(Wall {
                    kind: WallKind::Solid,
                    facing_sector: rs,
                    sidedef: rside,
                    quad: quad(p1, p2, sec.floor_height as f32, sec.ceiling_height as f32),
                });
            }
        }
        (None, Some(ls)) => {
            if let (Some(sec), Some(lside)) = (map.sectors.get(ls), left_side) {
                out.push(Wall {
                    kind: WallKind::Solid,
                    facing_sector: ls,
                    sidedef: lside,
                    quad: quad(p2, p1, sec.floor_height as f32, sec.ceiling_height as f32),
                });
            }
        }
        (Some(rs), Some(ls)) => {
            let (Some(rsec), Some(lsec)) = (map.sectors.get(rs), map.sectors.get(ls)) else {
                return;
            };
            // Lower step: rendered on the side facing into the sector with the
            // taller floor; the texture comes from that side's sidedef.
            if rsec.floor_height < lsec.floor_height {
                if let Some(lside) = left_side {
                    out.push(Wall {
                        kind: WallKind::Lower,
                        facing_sector: ls,
                        sidedef: lside,
                        quad: quad(p2, p1, rsec.floor_height as f32, lsec.floor_height as f32),
                    });
                }
            } else if lsec.floor_height < rsec.floor_height {
                if let Some(rside) = right_side {
                    out.push(Wall {
                        kind: WallKind::Lower,
                        facing_sector: rs,
                        sidedef: rside,
                        quad: quad(p1, p2, lsec.floor_height as f32, rsec.floor_height as f32),
                    });
                }
            }
            // Upper step.
            if rsec.ceiling_height > lsec.ceiling_height {
                if let Some(lside) = left_side {
                    out.push(Wall {
                        kind: WallKind::Upper,
                        facing_sector: ls,
                        sidedef: lside,
                        quad: quad(
                            p2,
                            p1,
                            lsec.ceiling_height as f32,
                            rsec.ceiling_height as f32,
                        ),
                    });
                }
            } else if lsec.ceiling_height > rsec.ceiling_height {
                if let Some(rside) = right_side {
                    out.push(Wall {
                        kind: WallKind::Upper,
                        facing_sector: rs,
                        sidedef: rside,
                        quad: quad(
                            p1,
                            p2,
                            rsec.ceiling_height as f32,
                            lsec.ceiling_height as f32,
                        ),
                    });
                }
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
