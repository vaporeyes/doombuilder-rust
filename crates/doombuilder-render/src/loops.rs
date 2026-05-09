// ABOUTME: Recover closed edge loops for each sector by walking linedef/sidedef
// ABOUTME: graphs. Tolerant of T-junctions and unclosed loops; returns what it can.

use std::collections::{HashMap, HashSet};

use doombuilder_core::map::{Map, SectorId, VertexId};

/// All closed loops belonging to one sector. Each loop is an ordered list of
/// vertex IDs; loops are not winding-canonicalised — downstream triangulation
/// uses even-odd point-in-polygon to handle holes regardless of orientation.
#[derive(Debug, Clone, Default)]
pub struct SectorLoops {
    pub loops: Vec<Vec<VertexId>>,
    /// Edges that belong to this sector but couldn't be stitched into a closed
    /// loop. Surfaced for diagnostics; the rest of the sector is still drawable.
    pub orphan_edges: Vec<(VertexId, VertexId)>,
}

/// Extract sector loops for every sector in the map.
pub fn extract_sector_loops(map: &Map) -> HashMap<SectorId, SectorLoops> {
    let edges_by_sector = collect_sector_edges(map);
    edges_by_sector
        .into_iter()
        .map(|(sid, edges)| (sid, walk_loops(&edges)))
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    a: VertexId,
    b: VertexId,
}

fn collect_sector_edges(map: &Map) -> HashMap<SectorId, Vec<Edge>> {
    let mut out: HashMap<SectorId, Vec<Edge>> = HashMap::new();
    for (_lid, line) in &map.linedefs {
        if let Some(side_id) = line.right {
            if let Some(side) = map.sidedefs.get(side_id) {
                out.entry(side.sector).or_default().push(Edge {
                    a: line.v1,
                    b: line.v2,
                });
            }
        }
        if let Some(side_id) = line.left {
            if let Some(side) = map.sidedefs.get(side_id) {
                out.entry(side.sector).or_default().push(Edge {
                    a: line.v1,
                    b: line.v2,
                });
            }
        }
    }
    out
}

fn walk_loops(edges: &[Edge]) -> SectorLoops {
    if edges.is_empty() {
        return SectorLoops::default();
    }

    // adjacency: vertex -> Vec<(other_vertex, edge_index)>
    let mut adj: HashMap<VertexId, Vec<(VertexId, usize)>> = HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        adj.entry(e.a).or_default().push((e.b, i));
        adj.entry(e.b).or_default().push((e.a, i));
    }

    let mut visited = HashSet::with_capacity(edges.len());
    let mut loops = Vec::new();
    let mut orphans = Vec::new();

    for start_idx in 0..edges.len() {
        if visited.contains(&start_idx) {
            continue;
        }
        match walk_one_loop(edges, &adj, &mut visited, start_idx) {
            Ok(l) => loops.push(l),
            Err(es) => orphans.extend(es),
        }
    }

    SectorLoops {
        loops,
        orphan_edges: orphans,
    }
}

/// Try to follow edges from `start_idx` until we return to the start vertex,
/// marking edges visited. On success returns the closed loop; on failure
/// returns the (still-marked-visited) edges as orphans for diagnostics.
fn walk_one_loop(
    edges: &[Edge],
    adj: &HashMap<VertexId, Vec<(VertexId, usize)>>,
    visited: &mut HashSet<usize>,
    start_idx: usize,
) -> Result<Vec<VertexId>, Vec<(VertexId, VertexId)>> {
    let start_edge = edges[start_idx];
    let start_vertex = start_edge.a;
    let mut current_vertex = start_edge.b;
    let mut path = vec![start_vertex, current_vertex];
    visited.insert(start_idx);
    let mut walked_edges = vec![start_idx];

    loop {
        if current_vertex == start_vertex {
            // Closed loop. Drop the duplicated start vertex at the end.
            path.pop();
            return Ok(path);
        }
        let Some(neighbours) = adj.get(&current_vertex) else {
            return Err(orphan_edges(edges, &walked_edges));
        };
        let next = neighbours
            .iter()
            .find(|(_, idx)| !visited.contains(idx))
            .copied();
        let Some((next_vertex, next_idx)) = next else {
            return Err(orphan_edges(edges, &walked_edges));
        };
        visited.insert(next_idx);
        walked_edges.push(next_idx);
        path.push(next_vertex);
        current_vertex = next_vertex;

        if path.len() > edges.len() + 1 {
            // Pathological: should never run longer than the edge set itself.
            return Err(orphan_edges(edges, &walked_edges));
        }
    }
}

fn orphan_edges(edges: &[Edge], idxs: &[usize]) -> Vec<(VertexId, VertexId)> {
    idxs.iter().map(|&i| (edges[i].a, edges[i].b)).collect()
}
