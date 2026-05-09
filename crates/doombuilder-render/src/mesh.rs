// ABOUTME: Triangulate a sector's loops into a renderable floor mesh using
// ABOUTME: spade's Constrained Delaunay Triangulation. Even-odd test discards
// ABOUTME: triangles outside the polygon and inside holes.

use doombuilder_core::map::{Map, SectorId, VertexId};
use spade::{
    handles::FixedVertexHandle, ConstrainedDelaunayTriangulation, InsertionError, Point2,
    Triangulation,
};
use thiserror::Error;

use crate::loops::SectorLoops;

#[derive(Debug, Error)]
pub enum MeshError {
    #[error("sector has no closed loops")]
    NoLoops,

    #[error("CDT insertion failed: {0:?}")]
    Insertion(InsertionError),

    #[error("vertex {0:?} not present in map")]
    MissingVertex(VertexId),
}

/// Triangulated floor (or ceiling — same XY mesh, different Z) for one sector.
#[derive(Debug, Clone, Default)]
pub struct FloorMesh {
    /// XY positions in map units. Y is map-space (positive north).
    pub positions: Vec<[f32; 2]>,
    /// CCW triangle indices into `positions`.
    pub indices: Vec<u32>,
}

pub fn triangulate_sector(
    map: &Map,
    sector: SectorId,
    loops: &SectorLoops,
) -> Result<FloorMesh, MeshError> {
    if loops.loops.is_empty() {
        return Err(MeshError::NoLoops);
    }

    // Resolve VertexIds → coordinates and assign each unique vertex a local
    // index. Spade also dedupes on insert but going through our own table makes
    // the index buffer match the insertion order.
    let mut positions: Vec<[f32; 2]> = Vec::new();
    let mut local_index: std::collections::HashMap<VertexId, usize> =
        std::collections::HashMap::new();

    let mut intern = |vid: VertexId| -> Result<usize, MeshError> {
        if let Some(&i) = local_index.get(&vid) {
            return Ok(i);
        }
        let v = map.vertices.get(vid).ok_or(MeshError::MissingVertex(vid))?;
        let i = positions.len();
        positions.push([v.x as f32, v.y as f32]);
        local_index.insert(vid, i);
        Ok(i)
    };

    // First pass: intern every vertex referenced by any loop.
    let local_loops: Vec<Vec<usize>> = loops
        .loops
        .iter()
        .map(|loop_| loop_.iter().map(|&v| intern(v)).collect::<Result<_, _>>())
        .collect::<Result<_, _>>()?;

    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::new();
    let mut handles: Vec<FixedVertexHandle> = Vec::with_capacity(positions.len());
    for p in &positions {
        let h = cdt
            .insert(Point2::new(p[0] as f64, p[1] as f64))
            .map_err(MeshError::Insertion)?;
        handles.push(h);
    }

    // Second pass: add each loop edge as a constraint segment.
    for loop_ in &local_loops {
        if loop_.len() < 3 {
            continue;
        }
        for window in 0..loop_.len() {
            let a = loop_[window];
            let b = loop_[(window + 1) % loop_.len()];
            if a == b {
                continue;
            }
            let ha = handles[a];
            let hb = handles[b];
            // can_add_constraint avoids panics from intersecting constraints;
            // we silently skip pathological cases (Doom permits self-intersection).
            if cdt.can_add_constraint(ha, hb) {
                cdt.add_constraint(ha, hb);
            }
        }
    }
    let _ = sector;

    // Third pass: collect inner-face triangles whose centroid lies inside the
    // polygon (even-odd test against all loops).
    let mut indices: Vec<u32> = Vec::new();
    for face in cdt.inner_faces() {
        let verts = face.vertices();
        let p0 = verts[0].position();
        let p1 = verts[1].position();
        let p2 = verts[2].position();
        let cx = (p0.x + p1.x + p2.x) / 3.0;
        let cy = (p0.y + p1.y + p2.y) / 3.0;
        if !point_inside_loops(&local_loops, &positions, cx as f32, cy as f32) {
            continue;
        }
        // Map each spade point back to our local index by exact match.
        let i0 = find_position(&positions, p0.x as f32, p0.y as f32);
        let i1 = find_position(&positions, p1.x as f32, p1.y as f32);
        let i2 = find_position(&positions, p2.x as f32, p2.y as f32);
        if let (Some(a), Some(b), Some(c)) = (i0, i1, i2) {
            indices.push(a as u32);
            indices.push(b as u32);
            indices.push(c as u32);
        }
    }

    Ok(FloorMesh { positions, indices })
}

fn point_inside_loops(loops: &[Vec<usize>], positions: &[[f32; 2]], x: f32, y: f32) -> bool {
    let mut inside = false;
    for loop_ in loops {
        let n = loop_.len();
        if n < 3 {
            continue;
        }
        let mut j = n - 1;
        for i in 0..n {
            let pi = positions[loop_[i]];
            let pj = positions[loop_[j]];
            let intersect = ((pi[1] > y) != (pj[1] > y))
                && (x < (pj[0] - pi[0]) * (y - pi[1]) / (pj[1] - pi[1] + f32::EPSILON) + pi[0]);
            if intersect {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

fn find_position(positions: &[[f32; 2]], x: f32, y: f32) -> Option<usize> {
    positions
        .iter()
        .position(|p| (p[0] - x).abs() < 1e-3 && (p[1] - y).abs() < 1e-3)
}
