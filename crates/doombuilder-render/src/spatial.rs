// ABOUTME: R-tree spatial index over map geometry for O(log N) hit-testing.
// ABOUTME: Indexes vertices (points), linedefs (segment AABBs), and sectors
// ABOUTME: (polygon AABBs) so hover and selection stay 60+ FPS on huge WADs.

use std::collections::HashMap;

use doombuilder_core::map::{LinedefId, Map, SectorId, ThingId, VertexId};
use rstar::{RTree, RTreeObject, AABB};

use crate::mesh::FloorMesh;

#[derive(Debug, Clone, Copy)]
pub enum Hit {
    Vertex(VertexId),
    Linedef(LinedefId),
    Sector(SectorId),
    Thing(ThingId),
}

#[derive(Debug)]
pub struct SpatialIndex {
    vertices: RTree<VertexNode>,
    linedefs: RTree<LinedefNode>,
    sectors: RTree<SectorBoundsNode>,
    things: RTree<ThingNode>,
    sector_meshes: HashMap<SectorId, FloorMesh>,
}

impl SpatialIndex {
    pub fn build(map: &Map, sector_meshes: Vec<(SectorId, FloorMesh)>) -> Self {
        let vertex_nodes: Vec<VertexNode> = map
            .vertices
            .iter()
            .map(|(id, v)| VertexNode {
                id,
                x: v.x as f32,
                y: v.y as f32,
            })
            .collect();

        let linedef_nodes: Vec<LinedefNode> = map
            .linedefs
            .iter()
            .filter_map(|(id, line)| {
                let v1 = map.vertices.get(line.v1)?;
                let v2 = map.vertices.get(line.v2)?;
                Some(LinedefNode {
                    id,
                    ax: v1.x as f32,
                    ay: v1.y as f32,
                    bx: v2.x as f32,
                    by: v2.y as f32,
                })
            })
            .collect();

        let sector_nodes: Vec<SectorBoundsNode> = sector_meshes
            .iter()
            .filter_map(|(sid, mesh)| {
                let bounds = mesh_aabb(mesh)?;
                Some(SectorBoundsNode {
                    id: *sid,
                    min: [bounds.0[0], bounds.0[1]],
                    max: [bounds.1[0], bounds.1[1]],
                })
            })
            .collect();

        let thing_nodes: Vec<ThingNode> = map
            .things
            .iter()
            .map(|(id, t)| ThingNode {
                id,
                x: t.x as f32,
                y: t.y as f32,
            })
            .collect();

        let sector_meshes_by_id = sector_meshes.into_iter().collect();

        Self {
            vertices: RTree::bulk_load(vertex_nodes),
            linedefs: RTree::bulk_load(linedef_nodes),
            sectors: RTree::bulk_load(sector_nodes),
            things: RTree::bulk_load(thing_nodes),
            sector_meshes: sector_meshes_by_id,
        }
    }

    /// Vertex closest to (x, y), but only if within `max_distance` world units.
    /// O(log N) via R-tree nearest_neighbor.
    pub fn nearest_vertex(&self, x: f32, y: f32, max_distance: f32) -> Option<VertexId> {
        let node = self.vertices.nearest_neighbor(&[x, y])?;
        let dx = node.x - x;
        let dy = node.y - y;
        if dx * dx + dy * dy <= max_distance * max_distance {
            Some(node.id)
        } else {
            None
        }
    }

    /// Linedef whose segment is closest to (x, y), within `max_distance`.
    /// AABB intersection via R-tree, then exact segment-distance check on the
    /// candidates. Worst case O(k log N) where k is the candidate count, which
    /// is bounded by linedefs per leaf, not total count.
    pub fn nearest_linedef(&self, x: f32, y: f32, max_distance: f32) -> Option<LinedefId> {
        let r = max_distance;
        let envelope = AABB::from_corners([x - r, y - r], [x + r, y + r]);
        let mut best: Option<(LinedefId, f32)> = None;
        for node in self.linedefs.locate_in_envelope_intersecting(&envelope) {
            let d2 = segment_distance_squared(node.ax, node.ay, node.bx, node.by, x, y);
            if d2 > r * r {
                continue;
            }
            match best {
                Some((_, b)) if b <= d2 => {}
                _ => best = Some((node.id, d2)),
            }
        }
        best.map(|(id, _)| id)
    }

    /// Sector whose triangulated mesh contains (x, y), if any. Picks the
    /// smallest-area sector when multiple match (handles nested sectors / holes
    /// reasonably; full Doom semantics need explicit containment trees).
    pub fn sector_at(&self, x: f32, y: f32) -> Option<SectorId> {
        let envelope = AABB::from_point([x, y]);
        let mut best: Option<(SectorId, f32)> = None;
        for node in self.sectors.locate_in_envelope_intersecting(&envelope) {
            let mesh = &self.sector_meshes[&node.id];
            if !mesh_contains_point(mesh, x, y) {
                continue;
            }
            let area = (node.max[0] - node.min[0]) * (node.max[1] - node.min[1]);
            match best {
                Some((_, a)) if a <= area => {}
                _ => best = Some((node.id, area)),
            }
        }
        best.map(|(id, _)| id)
    }

    /// All vertices whose point lies inside the world-space rectangle.
    pub fn vertices_in_rect(&self, min: [f32; 2], max: [f32; 2]) -> Vec<VertexId> {
        let envelope = AABB::from_corners(min, max);
        self.vertices
            .locate_in_envelope(&envelope)
            .map(|n| n.id)
            .collect()
    }

    /// All linedefs whose BOTH endpoints lie inside the world-space rectangle.
    /// "Fully contained" matches the convention used by classic Doom editors.
    pub fn linedefs_in_rect(&self, min: [f32; 2], max: [f32; 2]) -> Vec<LinedefId> {
        let envelope = AABB::from_corners(min, max);
        self.linedefs
            .locate_in_envelope_intersecting(&envelope)
            .filter(|n| {
                point_in_rect(n.ax, n.ay, min, max) && point_in_rect(n.bx, n.by, min, max)
            })
            .map(|n| n.id)
            .collect()
    }

    /// Thing closest to (x, y), within `max_distance` world units.
    pub fn nearest_thing(&self, x: f32, y: f32, max_distance: f32) -> Option<ThingId> {
        let node = self.things.nearest_neighbor(&[x, y])?;
        let dx = node.x - x;
        let dy = node.y - y;
        if dx * dx + dy * dy <= max_distance * max_distance {
            Some(node.id)
        } else {
            None
        }
    }

    /// Top-level hit test. Things sit on top of everything else (their
    /// pickable radius is fixed in world units), then standard
    /// vertex > linedef > sector priority.
    pub fn hit_test(
        &self,
        x: f32,
        y: f32,
        vertex_radius: f32,
        linedef_radius: f32,
    ) -> Option<Hit> {
        if let Some(t) = self.nearest_thing(x, y, 24.0) {
            return Some(Hit::Thing(t));
        }
        if let Some(v) = self.nearest_vertex(x, y, vertex_radius) {
            return Some(Hit::Vertex(v));
        }
        if let Some(l) = self.nearest_linedef(x, y, linedef_radius) {
            return Some(Hit::Linedef(l));
        }
        self.sector_at(x, y).map(Hit::Sector)
    }
}

#[derive(Debug, Clone, Copy)]
struct ThingNode {
    id: ThingId,
    x: f32,
    y: f32,
}

impl RTreeObject for ThingNode {
    type Envelope = AABB<[f32; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_point([self.x, self.y])
    }
}

impl rstar::PointDistance for ThingNode {
    fn distance_2(&self, point: &[f32; 2]) -> f32 {
        let dx = self.x - point[0];
        let dy = self.y - point[1];
        dx * dx + dy * dy
    }
}

#[derive(Debug, Clone, Copy)]
struct VertexNode {
    id: VertexId,
    x: f32,
    y: f32,
}

impl RTreeObject for VertexNode {
    type Envelope = AABB<[f32; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_point([self.x, self.y])
    }
}

impl rstar::PointDistance for VertexNode {
    fn distance_2(&self, point: &[f32; 2]) -> f32 {
        let dx = self.x - point[0];
        let dy = self.y - point[1];
        dx * dx + dy * dy
    }
}

#[derive(Debug, Clone, Copy)]
struct LinedefNode {
    id: LinedefId,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
}

impl RTreeObject for LinedefNode {
    type Envelope = AABB<[f32; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.ax.min(self.bx), self.ay.min(self.by)], [
            self.ax.max(self.bx),
            self.ay.max(self.by),
        ])
    }
}

#[derive(Debug, Clone, Copy)]
struct SectorBoundsNode {
    id: SectorId,
    min: [f32; 2],
    max: [f32; 2],
}

impl RTreeObject for SectorBoundsNode {
    type Envelope = AABB<[f32; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(self.min, self.max)
    }
}

fn point_in_rect(px: f32, py: f32, min: [f32; 2], max: [f32; 2]) -> bool {
    px >= min[0] && px <= max[0] && py >= min[1] && py <= max[1]
}

fn segment_distance_squared(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    if len_sq <= f32::EPSILON {
        let ex = px - ax;
        let ey = py - ay;
        return ex * ex + ey * ey;
    }
    let t = ((px - ax) * dx + (py - ay) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    let ex = px - cx;
    let ey = py - cy;
    ex * ex + ey * ey
}

fn mesh_aabb(mesh: &FloorMesh) -> Option<([f32; 2], [f32; 2])> {
    let mut iter = mesh.positions.iter().copied();
    let first = iter.next()?;
    let mut min = first;
    let mut max = first;
    for p in iter {
        if p[0] < min[0] {
            min[0] = p[0];
        }
        if p[1] < min[1] {
            min[1] = p[1];
        }
        if p[0] > max[0] {
            max[0] = p[0];
        }
        if p[1] > max[1] {
            max[1] = p[1];
        }
    }
    Some((min, max))
}

fn mesh_contains_point(mesh: &FloorMesh, x: f32, y: f32) -> bool {
    let mut i = 0;
    while i + 2 < mesh.indices.len() {
        let a = mesh.positions[mesh.indices[i] as usize];
        let b = mesh.positions[mesh.indices[i + 1] as usize];
        let c = mesh.positions[mesh.indices[i + 2] as usize];
        if point_in_triangle(x, y, a, b, c) {
            return true;
        }
        i += 3;
    }
    false
}

fn point_in_triangle(px: f32, py: f32, a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let d1 = sign(px, py, a, b);
    let d2 = sign(px, py, b, c);
    let d3 = sign(px, py, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

fn sign(px: f32, py: f32, a: [f32; 2], b: [f32; 2]) -> f32 {
    (px - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (py - b[1])
}
