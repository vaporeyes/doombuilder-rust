// ABOUTME: Vanilla Doom node builder. Produces SEGS / SSECTORS / NODES via a
// ABOUTME: classic linedef-partitioner BSP, plus BLOCKMAP and a zero REJECT.

use std::collections::HashMap;

use crate::map::{LinedefId, Map, VertexId};

/// Full set of binary lumps a vanilla Doom engine needs alongside the base
/// THINGS/LINEDEFS/SIDEDEFS/VERTEXES/SECTORS lumps.
#[derive(Debug, Default)]
pub struct NodeOutput {
    /// Extra (x,y) vertices created by partition splits. Appended after the
    /// base map's vertices in the VERTEXES lump; indexed N..N+extra.len() in
    /// the SEGS lump.
    pub extra_vertices: Vec<(i16, i16)>,
    pub segs: Vec<u8>,
    pub ssectors: Vec<u8>,
    pub nodes: Vec<u8>,
    pub blockmap: Vec<u8>,
    pub reject: Vec<u8>,
}

const BLOCK_SIZE: i32 = 128;
const SPLIT_WEIGHT: i32 = 8;

/// Build all derived lumps for a map. Returns empty lump bodies for empty
/// inputs (no segs => still legal empty SSECTORS/NODES).
pub fn build_nodes(map: &Map) -> NodeOutput {
    let vertex_idx: HashMap<VertexId, u16> = map
        .vertices
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i as u16))
        .collect();
    let linedef_idx: HashMap<LinedefId, u16> = map
        .linedefs
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i as u16))
        .collect();

    let base_vertex_count = map.vertices.len() as u32;
    let mut ctx = BuildCtx {
        base_vertex_count,
        extra_vertices: Vec::new(),
        segs_out: Vec::new(),
        ssectors_out: Vec::new(),
        nodes_out: Vec::new(),
    };

    let initial_segs = build_initial_segs(map, &vertex_idx, &linedef_idx);

    let mut root: Option<u16> = None;
    if !initial_segs.is_empty() {
        root = Some(build_bsp(&mut ctx, initial_segs));
    }

    // If the entire map is convex, build_bsp produced 0 nodes and a single
    // ssector; the engine reads `nodes[numnodes - 1]` to find the BSP root
    // and underflows when `numnodes == 0`. Synthesize a degenerate root node
    // whose left + right children both point at the lone ssector.
    if ctx.nodes_out.is_empty() && !ctx.ssectors_out.is_empty() {
        let leaf_child = root.unwrap_or(0x8000); // ssector 0 with high bit set
        let bbox = map_bbox(map);
        ctx.nodes_out.push(OutNode {
            px: bbox.left,
            py: bbox.bottom,
            // Non-zero direction so engines that consult the partition line
            // don't trip on a zero-vector divisor.
            pdx: 1,
            pdy: 0,
            right_bb: bbox,
            left_bb: bbox,
            right_child: leaf_child,
            left_child: leaf_child,
        });
    }

    let segs = encode_segs(&ctx.segs_out);
    let ssectors = encode_ssectors(&ctx.ssectors_out);
    let nodes = encode_nodes(&ctx.nodes_out);
    let blockmap = build_blockmap(map, &vertex_idx);
    let reject = build_reject(map.sectors.len());

    NodeOutput {
        extra_vertices: ctx.extra_vertices,
        segs,
        ssectors,
        nodes,
        blockmap,
        reject,
    }
}

#[derive(Debug, Clone)]
struct Seg {
    sx: i32,
    sy: i32,
    ex: i32,
    ey: i32,
    start_vid: u32,
    end_vid: u32,
    linedef: u16,
    side: u16, // 0 = right (front), 1 = left (back)
    /// Distance from linedef.v1 along the linedef to seg.start, in map units.
    offset: i32,
    /// (lx, ly, ldx, ldy) of the underlying linedef, kept so we can resolve
    /// the "offset" of new split midpoints back to linedef.v1.
    ldx: i32,
    ldy: i32,
    lx: i32,
    ly: i32,
}

struct BuildCtx {
    base_vertex_count: u32,
    extra_vertices: Vec<(i16, i16)>,
    segs_out: Vec<OutSeg>,
    ssectors_out: Vec<(u16, u16)>, // (count, first)
    nodes_out: Vec<OutNode>,
}

#[derive(Debug, Clone, Copy)]
struct OutSeg {
    start_vid: u16,
    end_vid: u16,
    angle: i16,
    linedef: u16,
    side: u16,
    offset: i16,
}

#[derive(Debug, Clone, Copy)]
struct OutNode {
    px: i16,
    py: i16,
    pdx: i16,
    pdy: i16,
    right_bb: BBox,
    left_bb: BBox,
    right_child: u16,
    left_child: u16,
}

#[derive(Debug, Clone, Copy)]
struct BBox {
    /// top, bottom, left, right — Doom's NODES order is (top, bottom, left, right).
    top: i16,
    bottom: i16,
    left: i16,
    right: i16,
}

fn build_initial_segs(
    map: &Map,
    vertex_idx: &HashMap<VertexId, u16>,
    linedef_idx: &HashMap<LinedefId, u16>,
) -> Vec<Seg> {
    let mut out = Vec::new();
    for (lid, line) in &map.linedefs {
        let (Some(v1), Some(v2)) =
            (map.vertices.get(line.v1), map.vertices.get(line.v2))
        else {
            continue;
        };
        let lidx = linedef_idx[&lid];
        let v1_idx = vertex_idx[&line.v1] as u32;
        let v2_idx = vertex_idx[&line.v2] as u32;
        let ldx = v2.x - v1.x;
        let ldy = v2.y - v1.y;
        let llen = ((ldx as f64).hypot(ldy as f64)) as i32;
        let _ = llen;
        if line.right.is_some() {
            out.push(Seg {
                sx: v1.x,
                sy: v1.y,
                ex: v2.x,
                ey: v2.y,
                start_vid: v1_idx,
                end_vid: v2_idx,
                linedef: lidx,
                side: 0,
                offset: 0,
                ldx,
                ldy,
                lx: v1.x,
                ly: v1.y,
            });
        }
        if line.left.is_some() {
            // Left-side seg goes v2 -> v1; its start_vid is v2; offset measured
            // from linedef.v1, so its start sits one linedef-length away.
            let llen = ((ldx as f64).hypot(ldy as f64)).round() as i32;
            out.push(Seg {
                sx: v2.x,
                sy: v2.y,
                ex: v1.x,
                ey: v1.y,
                start_vid: v2_idx,
                end_vid: v1_idx,
                linedef: lidx,
                side: 1,
                offset: llen,
                ldx,
                ldy,
                lx: v1.x,
                ly: v1.y,
            });
        }
    }
    out
}

/// Returns the child reference for this subtree:
///   - high bit set + ssector index, or
///   - node index (no high bit) when an internal node was emitted.
fn build_bsp(ctx: &mut BuildCtx, segs: Vec<Seg>) -> u16 {
    if segs.is_empty() {
        // Defensive: emit an empty ssector so callers always have a child to point at.
        let first = ctx.segs_out.len() as u16;
        ctx.ssectors_out.push((0, first));
        let idx = (ctx.ssectors_out.len() - 1) as u16;
        return idx | 0x8000;
    }

    match pick_partition(&segs) {
        Some(part_idx) => {
            let partition = segs[part_idx].clone();
            let (front, back) = split_segs(segs, &partition, ctx);

            // No real split happened (degenerate scoring): make a leaf.
            if front.is_empty() || back.is_empty() {
                return make_ssector(ctx, if front.is_empty() { back } else { front });
            }

            let right_bb = bbox_of(&front);
            let left_bb = bbox_of(&back);
            let right_child = build_bsp(ctx, front);
            let left_child = build_bsp(ctx, back);

            let node = OutNode {
                px: clamp_i16(partition.sx),
                py: clamp_i16(partition.sy),
                pdx: clamp_i16(partition.ex - partition.sx),
                pdy: clamp_i16(partition.ey - partition.sy),
                right_bb,
                left_bb,
                right_child,
                left_child,
            };
            ctx.nodes_out.push(node);
            (ctx.nodes_out.len() - 1) as u16
        }
        None => make_ssector(ctx, segs),
    }
}

fn make_ssector(ctx: &mut BuildCtx, segs: Vec<Seg>) -> u16 {
    let first = ctx.segs_out.len() as u16;
    for s in &segs {
        ctx.segs_out.push(OutSeg {
            start_vid: s.start_vid as u16,
            end_vid: s.end_vid as u16,
            angle: bam_angle(s.ex - s.sx, s.ey - s.sy),
            linedef: s.linedef,
            side: s.side,
            offset: clamp_i16(s.offset),
        });
    }
    let count = segs.len() as u16;
    ctx.ssectors_out.push((count, first));
    let idx = (ctx.ssectors_out.len() - 1) as u16;
    idx | 0x8000
}

/// Try every seg as partitioner; score by tree balance + split count. Returns
/// `None` if no seg yields both a non-empty front and non-empty back, meaning
/// the input is already convex (leaf).
fn pick_partition(segs: &[Seg]) -> Option<usize> {
    let mut best: Option<(i32, usize)> = None;
    for (i, candidate) in segs.iter().enumerate() {
        let mut front: i32 = 0;
        let mut back: i32 = 0;
        let mut splits: i32 = 0;
        for (j, other) in segs.iter().enumerate() {
            if i == j {
                front += 1;
                continue;
            }
            let s1 = side_of(candidate, other.sx, other.sy);
            let s2 = side_of(candidate, other.ex, other.ey);
            match classify(s1, s2) {
                Class::Front => front += 1,
                Class::Back => back += 1,
                Class::Split => {
                    front += 1;
                    back += 1;
                    splits += 1;
                }
                Class::Collinear => {
                    // Same-direction collinear → front; opposite → back.
                    let dot = (other.ex - other.sx) * (candidate.ex - candidate.sx)
                        + (other.ey - other.sy) * (candidate.ey - candidate.sy);
                    if dot >= 0 {
                        front += 1;
                    } else {
                        back += 1;
                    }
                }
            }
        }
        if front == 0 || back == 0 {
            continue;
        }
        let score = (front - back).abs() + splits * SPLIT_WEIGHT;
        if best.map(|(s, _)| score < s).unwrap_or(true) {
            best = Some((score, i));
        }
    }
    best.map(|(_, i)| i)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Class {
    Front,
    Back,
    Split,
    Collinear,
}

fn classify(s1: i64, s2: i64) -> Class {
    if s1 == 0 && s2 == 0 {
        Class::Collinear
    } else if s1 >= 0 && s2 >= 0 {
        Class::Front
    } else if s1 <= 0 && s2 <= 0 {
        Class::Back
    } else {
        Class::Split
    }
}

/// Cross product of partition direction and (point - partition.start).
/// Positive ⇒ front (right side), negative ⇒ back (left side), zero ⇒ on line.
fn side_of(part: &Seg, x: i32, y: i32) -> i64 {
    let pdx = (part.ex - part.sx) as i64;
    let pdy = (part.ey - part.sy) as i64;
    let dx = (x - part.sx) as i64;
    let dy = (y - part.sy) as i64;
    pdx * dy - pdy * dx
}

fn split_segs(segs: Vec<Seg>, part: &Seg, ctx: &mut BuildCtx) -> (Vec<Seg>, Vec<Seg>) {
    let mut front: Vec<Seg> = Vec::new();
    let mut back: Vec<Seg> = Vec::new();
    for s in segs {
        let s1 = side_of(part, s.sx, s.sy);
        let s2 = side_of(part, s.ex, s.ey);
        match classify(s1, s2) {
            Class::Front => front.push(s),
            Class::Back => back.push(s),
            Class::Collinear => {
                let dot = (s.ex - s.sx) * (part.ex - part.sx)
                    + (s.ey - s.sy) * (part.ey - part.sy);
                if dot >= 0 {
                    front.push(s);
                } else {
                    back.push(s);
                }
            }
            Class::Split => {
                let (a, b) = split_one(&s, part, ctx);
                // a stays with s1 sign, b with s2 sign.
                if s1 > 0 {
                    front.push(a);
                    back.push(b);
                } else {
                    back.push(a);
                    front.push(b);
                }
            }
        }
    }
    (front, back)
}

fn split_one(seg: &Seg, part: &Seg, ctx: &mut BuildCtx) -> (Seg, Seg) {
    // Solve for parametric t along seg where it crosses partition's line.
    let pdx = (part.ex - part.sx) as f64;
    let pdy = (part.ey - part.sy) as f64;
    let sdx = (seg.ex - seg.sx) as f64;
    let sdy = (seg.ey - seg.sy) as f64;
    let denom = pdy * sdx - pdx * sdy;
    let (ix, iy) = if denom.abs() < 1e-9 {
        // Shouldn't happen for genuine Split, but fall back to midpoint.
        ((seg.sx + seg.ex) / 2, (seg.sy + seg.ey) / 2)
    } else {
        let t = (pdx * (seg.sy - part.sy) as f64 - pdy * (seg.sx - part.sx) as f64) / denom;
        let x = seg.sx as f64 + t * sdx;
        let y = seg.sy as f64 + t * sdy;
        (x.round() as i32, y.round() as i32)
    };

    let new_vid =
        ctx.base_vertex_count + ctx.extra_vertices.len() as u32;
    ctx.extra_vertices
        .push((clamp_i16(ix), clamp_i16(iy)));

    // New offset for the back half: distance along linedef from (lx,ly) to (ix,iy).
    let half_offset = {
        let llx = seg.ldx as f64;
        let lly = seg.ldy as f64;
        let llen = (llx * llx + lly * lly).sqrt().max(1e-9);
        let ux = llx / llen;
        let uy = lly / llen;
        let off = ((ix - seg.lx) as f64) * ux + ((iy - seg.ly) as f64) * uy;
        off.round() as i32
    };

    let a = Seg {
        sx: seg.sx,
        sy: seg.sy,
        ex: ix,
        ey: iy,
        start_vid: seg.start_vid,
        end_vid: new_vid,
        offset: seg.offset,
        ..seg.clone()
    };
    let b = Seg {
        sx: ix,
        sy: iy,
        ex: seg.ex,
        ey: seg.ey,
        start_vid: new_vid,
        end_vid: seg.end_vid,
        offset: half_offset,
        ..seg.clone()
    };
    (a, b)
}

/// Bounding box over every vertex in the map. Used as a fallback child bbox
/// for the synthetic root node when the BSP yields a single ssector.
fn map_bbox(map: &Map) -> BBox {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for (_, v) in &map.vertices {
        min_x = min_x.min(v.x);
        min_y = min_y.min(v.y);
        max_x = max_x.max(v.x);
        max_y = max_y.max(v.y);
    }
    if min_x > max_x {
        // Empty map fallback.
        return BBox { top: 0, bottom: 0, left: 0, right: 0 };
    }
    BBox {
        top: clamp_i16(max_y),
        bottom: clamp_i16(min_y),
        left: clamp_i16(min_x),
        right: clamp_i16(max_x),
    }
}

fn bbox_of(segs: &[Seg]) -> BBox {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for s in segs {
        min_x = min_x.min(s.sx).min(s.ex);
        min_y = min_y.min(s.sy).min(s.ey);
        max_x = max_x.max(s.sx).max(s.ex);
        max_y = max_y.max(s.sy).max(s.ey);
    }
    BBox {
        top: clamp_i16(max_y),
        bottom: clamp_i16(min_y),
        left: clamp_i16(min_x),
        right: clamp_i16(max_x),
    }
}

fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn bam_angle(dx: i32, dy: i32) -> i16 {
    let rad = (dy as f64).atan2(dx as f64);
    let bam = (rad / std::f64::consts::TAU * 65536.0).rem_euclid(65536.0);
    // Stored as i16 bit pattern; cast through u16.
    (bam.round() as u32 as u16) as i16
}

fn encode_segs(segs: &[OutSeg]) -> Vec<u8> {
    let mut out = Vec::with_capacity(segs.len() * 12);
    for s in segs {
        out.extend_from_slice(&s.start_vid.to_le_bytes());
        out.extend_from_slice(&s.end_vid.to_le_bytes());
        out.extend_from_slice(&s.angle.to_le_bytes());
        out.extend_from_slice(&s.linedef.to_le_bytes());
        out.extend_from_slice(&s.side.to_le_bytes());
        out.extend_from_slice(&s.offset.to_le_bytes());
    }
    out
}

fn encode_ssectors(ss: &[(u16, u16)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ss.len() * 4);
    for (count, first) in ss {
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&first.to_le_bytes());
    }
    out
}

fn encode_nodes(nodes: &[OutNode]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nodes.len() * 28);
    for n in nodes {
        out.extend_from_slice(&n.px.to_le_bytes());
        out.extend_from_slice(&n.py.to_le_bytes());
        out.extend_from_slice(&n.pdx.to_le_bytes());
        out.extend_from_slice(&n.pdy.to_le_bytes());
        for bb in [&n.right_bb, &n.left_bb] {
            out.extend_from_slice(&bb.top.to_le_bytes());
            out.extend_from_slice(&bb.bottom.to_le_bytes());
            out.extend_from_slice(&bb.left.to_le_bytes());
            out.extend_from_slice(&bb.right.to_le_bytes());
        }
        out.extend_from_slice(&n.right_child.to_le_bytes());
        out.extend_from_slice(&n.left_child.to_le_bytes());
    }
    out
}

fn build_blockmap(map: &Map, _vertex_idx: &HashMap<VertexId, u16>) -> Vec<u8> {
    if map.vertices.is_empty() {
        return Vec::new();
    }
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for (_, v) in &map.vertices {
        min_x = min_x.min(v.x);
        min_y = min_y.min(v.y);
        max_x = max_x.max(v.x);
        max_y = max_y.max(v.y);
    }
    // Round origin down to multiple of 8 (matches BSP convention; not strictly
    // required, but keeps fixed-point math clean for the engine).
    let origin_x = (min_x.div_euclid(8) * 8).max(i16::MIN as i32);
    let origin_y = (min_y.div_euclid(8) * 8).max(i16::MIN as i32);
    let cols = (((max_x - origin_x) / BLOCK_SIZE) + 1).max(1) as usize;
    let rows = (((max_y - origin_y) / BLOCK_SIZE) + 1).max(1) as usize;

    let mut buckets: Vec<Vec<u16>> = vec![Vec::new(); cols * rows];
    for (lid, line) in &map.linedefs {
        let _ = lid;
        let Some(v1) = map.vertices.get(line.v1) else {
            continue;
        };
        let Some(v2) = map.vertices.get(line.v2) else {
            continue;
        };
        let lindex = vertex_idx_to_linedef_idx(&map, line) as u16;
        // For each block cell whose AABB the segment intersects, append lindex.
        let lo_x = v1.x.min(v2.x);
        let hi_x = v1.x.max(v2.x);
        let lo_y = v1.y.min(v2.y);
        let hi_y = v1.y.max(v2.y);
        let bx0 = ((lo_x - origin_x).div_euclid(BLOCK_SIZE)).max(0) as usize;
        let by0 = ((lo_y - origin_y).div_euclid(BLOCK_SIZE)).max(0) as usize;
        let bx1 = (((hi_x - origin_x).div_euclid(BLOCK_SIZE)) as usize).min(cols - 1);
        let by1 = (((hi_y - origin_y).div_euclid(BLOCK_SIZE)) as usize).min(rows - 1);
        for by in by0..=by1 {
            for bx in bx0..=bx1 {
                let cell_x0 = origin_x + (bx as i32) * BLOCK_SIZE;
                let cell_y0 = origin_y + (by as i32) * BLOCK_SIZE;
                let cell_x1 = cell_x0 + BLOCK_SIZE;
                let cell_y1 = cell_y0 + BLOCK_SIZE;
                if segment_intersects_aabb(
                    v1.x, v1.y, v2.x, v2.y, cell_x0, cell_y0, cell_x1, cell_y1,
                ) {
                    buckets[by * cols + bx].push(lindex);
                }
            }
        }
    }

    // Header (4 i16) + cols*rows pointer table (u16) + per-block lists.
    let header_words = 4;
    let pointer_words = cols * rows;
    let mut offsets: Vec<u16> = vec![0; pointer_words];
    let mut lists: Vec<u16> = Vec::new();
    let lists_start_words = header_words + pointer_words;
    for (i, bucket) in buckets.iter().enumerate() {
        let here = lists_start_words + lists.len();
        offsets[i] = here as u16;
        lists.push(0x0000);
        for &li in bucket {
            lists.push(li);
        }
        lists.push(0xFFFF);
    }

    let total_words = lists_start_words + lists.len();
    let mut out = Vec::with_capacity(total_words * 2);
    out.extend_from_slice(&(origin_x as i16).to_le_bytes());
    out.extend_from_slice(&(origin_y as i16).to_le_bytes());
    out.extend_from_slice(&(cols as u16).to_le_bytes());
    out.extend_from_slice(&(rows as u16).to_le_bytes());
    for o in &offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    for w in &lists {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

/// Map a MapLinedef back to its serialization-order index by looking up its
/// position in the slotmap iteration. O(L) per call; tolerable since blockmap
/// build is itself O(L * cells).
fn vertex_idx_to_linedef_idx(map: &Map, target: &crate::map::MapLinedef) -> usize {
    // Compare by raw pointer equivalent isn't safe; use v1/v2/right/left as a
    // discriminator. Since we iterate linedefs in slotmap order, just match by
    // identity via address of `target`.
    for (i, (_, line)) in map.linedefs.iter().enumerate() {
        if std::ptr::eq(line, target) {
            return i;
        }
    }
    0
}

fn segment_intersects_aabb(
    sx: i32, sy: i32, ex: i32, ey: i32,
    rx0: i32, ry0: i32, rx1: i32, ry1: i32,
) -> bool {
    // Either endpoint inside?
    let inside = |x: i32, y: i32| x >= rx0 && x <= rx1 && y >= ry0 && y <= ry1;
    if inside(sx, sy) || inside(ex, ey) {
        return true;
    }
    // Liang-Barsky on the rectangle.
    let dx = (ex - sx) as f64;
    let dy = (ey - sy) as f64;
    let mut t0: f64 = 0.0;
    let mut t1: f64 = 1.0;
    let ps = [-dx, dx, -dy, dy];
    let qs = [
        (sx - rx0) as f64,
        (rx1 - sx) as f64,
        (sy - ry0) as f64,
        (ry1 - sy) as f64,
    ];
    for i in 0..4 {
        let p = ps[i];
        let q = qs[i];
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                if r > t1 {
                    return false;
                }
                if r > t0 {
                    t0 = r;
                }
            } else {
                if r < t0 {
                    return false;
                }
                if r < t1 {
                    t1 = r;
                }
            }
        }
    }
    t0 <= t1
}

/// Conservative all-zero REJECT: every sector pair "potentially visible".
/// Engines that consult REJECT for early monster sight rejection still work
/// correctly, just without the optimization.
fn build_reject(sector_count: usize) -> Vec<u8> {
    let bits = sector_count * sector_count;
    let bytes = (bits + 7) / 8;
    vec![0u8; bytes]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::MapFormat;
    use crate::map::{MapLinedef, MapSector, MapSidedef, MapVertex, TextureName};

    fn square_map() -> Map {
        let mut map = Map::new("E1M1", MapFormat::Doom);
        let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
        let v1 = map.vertices.insert(MapVertex { x: 128, y: 0 });
        let v2 = map.vertices.insert(MapVertex { x: 128, y: 128 });
        let v3 = map.vertices.insert(MapVertex { x: 0, y: 128 });
        let sec = map.sectors.insert(MapSector {
            floor_height: 0,
            ceiling_height: 128,
            floor_texture: TextureName([0; 8]),
            ceiling_texture: TextureName([0; 8]),
            light: 200,
            special: 0,
            tag: 0,
            sidedefs: Vec::new(),
        });
        let s = |sec| MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"WALL\0\0\0\0"),
        };
        let s0 = map.sidedefs.insert(s(sec));
        let s1 = map.sidedefs.insert(s(sec));
        let s2 = map.sidedefs.insert(s(sec));
        let s3 = map.sidedefs.insert(s(sec));
        let mk = |a, b, side| MapLinedef {
            v1: a,
            v2: b,
            flags: 0,
            special: 0,
            args: [0; 5],
            tag: 0,
            right: Some(side),
            left: None,
        };
        map.linedefs.insert(mk(v0, v1, s0));
        map.linedefs.insert(mk(v1, v2, s1));
        map.linedefs.insert(mk(v2, v3, s2));
        map.linedefs.insert(mk(v3, v0, s3));
        map
    }

    #[test]
    fn empty_map_produces_empty_outputs() {
        let map = Map::new("MAP01", MapFormat::Doom);
        let out = build_nodes(&map);
        assert!(out.segs.is_empty());
        assert!(out.ssectors.is_empty());
        assert!(out.nodes.is_empty());
        assert!(out.extra_vertices.is_empty());
        assert!(out.reject.is_empty());
    }

    #[test]
    fn convex_square_emits_one_ssector_with_synthetic_root() {
        let map = square_map();
        let out = build_nodes(&map);
        // 4 one-sided linedefs => 4 segs, all in one ssector.
        assert_eq!(out.segs.len(), 4 * 12, "4 segs of 12 bytes");
        assert_eq!(out.ssectors.len(), 4, "1 ssector of 4 bytes");
        // Convex maps need a synthetic root node so `nodes[numnodes - 1]`
        // is dereferenceable by the engine. Both children point at the leaf.
        assert_eq!(out.nodes.len(), 28, "one synthetic root node");
        assert!(out.extra_vertices.is_empty(), "no splits on convex map");
        // Node layout: partition (8b) + right_bb (8b) + left_bb (8b) +
        // right_child (2b) + left_child (2b) = 28 bytes. Children live at
        // offsets 24..26 and 26..28.
        let r = u16::from_le_bytes([out.nodes[24], out.nodes[25]]);
        let l = u16::from_le_bytes([out.nodes[26], out.nodes[27]]);
        assert!(r & 0x8000 != 0, "right child must be an ssector ref");
        assert!(l & 0x8000 != 0, "left child must be an ssector ref");
    }

    #[test]
    fn blockmap_header_and_pointer_table_present() {
        let map = square_map();
        let out = build_nodes(&map);
        assert!(out.blockmap.len() >= 8, "header is 8 bytes");
        // Parse header.
        let cols = u16::from_le_bytes([out.blockmap[4], out.blockmap[5]]) as usize;
        let rows = u16::from_le_bytes([out.blockmap[6], out.blockmap[7]]) as usize;
        assert!(cols >= 1 && rows >= 1);
        // Pointer table follows header.
        let expected_min = 8 + cols * rows * 2;
        assert!(out.blockmap.len() > expected_min);
    }

    #[test]
    fn reject_size_matches_sector_count() {
        let map = square_map();
        let out = build_nodes(&map);
        let s = map.sectors.len();
        let expected = (s * s + 7) / 8;
        assert_eq!(out.reject.len(), expected);
    }

    #[test]
    fn two_sector_map_partitions_and_emits_node() {
        // Two adjacent square sectors sharing one two-sided linedef in the middle.
        let mut map = Map::new("E1M2", MapFormat::Doom);
        let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
        let v1 = map.vertices.insert(MapVertex { x: 128, y: 0 });
        let v2 = map.vertices.insert(MapVertex { x: 256, y: 0 });
        let v3 = map.vertices.insert(MapVertex { x: 256, y: 128 });
        let v4 = map.vertices.insert(MapVertex { x: 128, y: 128 });
        let v5 = map.vertices.insert(MapVertex { x: 0, y: 128 });
        let sec_a = map.sectors.insert(MapSector {
            floor_height: 0,
            ceiling_height: 128,
            floor_texture: TextureName([0; 8]),
            ceiling_texture: TextureName([0; 8]),
            light: 200,
            special: 0,
            tag: 0,
            sidedefs: Vec::new(),
        });
        let sec_b = map.sectors.insert(MapSector {
            floor_height: 0,
            ceiling_height: 128,
            floor_texture: TextureName([0; 8]),
            ceiling_texture: TextureName([0; 8]),
            light: 200,
            special: 0,
            tag: 0,
            sidedefs: Vec::new(),
        });
        let mk_side = |sec| MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"WALL\0\0\0\0"),
        };
        // Sector A perimeter (v0->v1->v4->v5).
        let a_bottom = map.sidedefs.insert(mk_side(sec_a));
        let a_left = map.sidedefs.insert(mk_side(sec_a));
        let a_top = map.sidedefs.insert(mk_side(sec_a));
        // Sector B perimeter (v1->v2->v3->v4).
        let b_bottom = map.sidedefs.insert(mk_side(sec_b));
        let b_right = map.sidedefs.insert(mk_side(sec_b));
        let b_top = map.sidedefs.insert(mk_side(sec_b));
        // Shared two-sided line v1->v4.
        let mid_a = map.sidedefs.insert(mk_side(sec_a));
        let mid_b = map.sidedefs.insert(mk_side(sec_b));

        let mk = |v1, v2, right, left| MapLinedef {
            v1,
            v2,
            flags: 0,
            special: 0,
            args: [0; 5],
            tag: 0,
            right: Some(right),
            left,
        };
        map.linedefs.insert(mk(v0, v1, a_bottom, None));
        map.linedefs.insert(mk(v1, v2, b_bottom, None));
        map.linedefs.insert(mk(v2, v3, b_right, None));
        map.linedefs.insert(mk(v3, v4, b_top, None));
        map.linedefs.insert(mk(v4, v5, a_top, None));
        map.linedefs.insert(mk(v5, v0, a_left, None));
        // Two-sided in the middle: right = mid_b (faces into B), left = mid_a.
        map.linedefs.insert(mk(v1, v4, mid_b, Some(mid_a)));

        let out = build_nodes(&map);
        assert!(out.nodes.len() >= 28, "at least one internal node");
        // 6 single-sided + 1 two-sided => 8 segs minimum (before any extra splits).
        assert!(out.segs.len() >= 8 * 12);
        assert!(out.ssectors.len() >= 2 * 4, "at least 2 leaves");
    }
}
