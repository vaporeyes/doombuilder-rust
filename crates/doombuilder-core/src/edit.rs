// ABOUTME: Editing commands and an undo/redo stack. Commands are diffs that
// ABOUTME: can be applied or reverted in O(k) where k is the number of elements
// ABOUTME: touched. Snapshot-of-Map is intentionally avoided to keep undo cheap.

use crate::map::{
    LinedefId, Map, MapLinedef, MapSector, MapSidedef, MapThing, MapVertex, SectorId, SidedefId,
    TextureName, ThingId, VertexId,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct VertexMove {
    pub id: VertexId,
    pub dx: i32,
    pub dy: i32,
}

#[derive(Debug, Clone)]
pub struct ThingMove {
    pub id: ThingId,
    pub dx: i32,
    pub dy: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidedefSlot {
    Upper,
    Middle,
    Lower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectorSlot {
    Floor,
    Ceiling,
}

#[derive(Debug, Clone)]
pub enum Command {
    /// Translate one or more vertices by the given per-vertex deltas.
    MoveVertices(Vec<VertexMove>),
    /// Translate one or more things by the given per-thing deltas.
    MoveThings(Vec<ThingMove>),
    /// Replace one of a sidedef's three texture slot names.
    SetSidedefTexture {
        id: SidedefId,
        slot: SidedefSlot,
        old: TextureName,
        new: TextureName,
    },
    /// Replace a sector's floor or ceiling flat name.
    SetSectorTexture {
        id: SectorId,
        slot: SectorSlot,
        old: TextureName,
        new: TextureName,
    },
    /// Insert a new thing. `id` tracks the slotmap key for redo cycles; it is
    /// `Some` while the thing exists in the map and `None` after the create
    /// has been undone.
    CreateThing {
        id: Option<ThingId>,
        snapshot: MapThing,
    },
    /// Remove one or more things. Stores snapshots for re-insertion on undo;
    /// `current_ids` holds the ids of currently-inserted snapshots (empty
    /// after the delete has been applied).
    DeleteThings {
        snapshots: Vec<MapThing>,
        current_ids: Vec<ThingId>,
    },
    /// Change a linedef's action (special) value.
    SetLinedefSpecial {
        id: LinedefId,
        old: u16,
        new: u16,
    },
    /// Change a thing's kind (type id).
    SetThingKind {
        id: ThingId,
        old: u16,
        new: u16,
    },
    /// Bulk delete: vertices, linedefs, sidedefs, sectors, things.
    /// Snapshots carry the OLD ids so cross-references can be remapped on
    /// revert. `current_*` tracks the new ids after the most recent revert
    /// so a subsequent redo knows what to remove.
    DeleteElements(Box<DeletionState>),
    /// Add a chain of linedefs (and the new vertices they require). Symmetric
    /// of `DeleteElements`: revert removes by `current_*`; apply re-inserts
    /// from snapshots and rebuilds `current_*`.
    CreateLinedefChain(Box<LinedefChain>),
    /// Create a sector + per-linedef sidedef facing inward. Built from a
    /// closed loop of pre-existing linedefs that have no sides yet.
    MakeSector(Box<MakeSectorState>),
    /// Insert a midpoint vertex into one or more linedefs and split each into
    /// two. Sidedefs are cloned so both halves retain the same sectors.
    SplitLinedefs(Box<SplitLinedefsState>),
    /// Merge a set of vertices into one. Survivor moves to the centroid;
    /// linedef references are redirected; degenerate linedefs (v1==v2 after
    /// merge) and their sidedefs are removed.
    MergeVertices(Box<VertexMergeState>),
    /// Flip one or more linedefs: swap v1 <-> v2 and right <-> left sidedefs.
    /// Self-inverse, so apply and revert do the same work.
    FlipLinedefs(Vec<LinedefId>),
    /// Swap the right/left sidedef pointers without reversing the linedef
    /// direction. The line walks v1→v2 the same way after the flip, but
    /// whichever sector was on the "outside" now faces the player. Self-
    /// inverse like `FlipLinedefs`.
    FlipSidedefs(Vec<LinedefId>),
    /// Set sidedef texture offsets (per-sidedef X and/or Y) atomically. Used
    /// by Auto-Align Textures and other propagation tools.
    SetSidedefOffsets(Vec<SidedefOffsetChange>),
    /// Insert a previously-captured clipboard of map elements at a chosen
    /// offset. Undo removes them by their freshly-assigned ids.
    PasteClipboard(Box<PasteClipboardState>),
    /// Stitch pairs of overlapping (coincident-endpoint) linedefs. For each
    /// pair walked in opposite directions, the absorbed line's right sidedef
    /// is reassigned as the keeper's left sidedef and the absorbed line is
    /// removed, producing a single two-sided wall.
    StitchLines(Vec<StitchMerge>),
    /// Change one of a sector's integer fields.
    SetSectorIntField {
        id: SectorId,
        field: SectorIntField,
        old: i32,
        new: i32,
    },
    /// Change one of a linedef's integer fields (flags / tag).
    SetLinedefIntField {
        id: LinedefId,
        field: LinedefIntField,
        old: i32,
        new: i32,
    },
    /// Change one of a thing's integer fields (angle / flags).
    SetThingIntField {
        id: ThingId,
        field: ThingIntField,
        old: i32,
        new: i32,
    },
    /// Composite atomic op. Apply walks the vector forwards; revert walks it
    /// backwards. Used for gradients, "make door", and multi-sector edits.
    Batch(Vec<Command>),
    /// Merge sectors into a survivor. Sidedefs are re-pointed; merged sectors
    /// (and optionally their shared linedefs) are removed.
    JoinSectors(Box<JoinSectorsState>),
}

#[derive(Debug, Clone)]
pub struct StitchMerge {
    pub keeper: LinedefId,
    /// keeper.left before merging (typically `None`).
    pub keeper_old_left: Option<SidedefId>,
    /// keeper.left after merging — the absorbed line's right sidedef.
    pub keeper_new_left: Option<SidedefId>,
    /// Snapshot of the absorbed linedef for revert insertion.
    pub absorbed_line_id: LinedefId,
    pub absorbed_line_snap: crate::map::MapLinedef,
    /// If the absorbed line had a left sidedef, snapshot it (it gets
    /// removed during apply; revert re-inserts).
    pub absorbed_left_snap: Option<(SidedefId, crate::map::MapSidedef)>,
    /// Post-apply ids reused after a revert/re-apply round-trip.
    pub current_absorbed_line: Option<LinedefId>,
    pub current_absorbed_left: Option<SidedefId>,
}

#[derive(Debug, Clone)]
pub struct JoinSectorsState {
    pub survivor: SectorId,
    /// Original sectors being absorbed (snapshots for revert insertion).
    pub merged_snapshots: Vec<(SectorId, crate::map::MapSector)>,
    /// On reapply after undo, slotmap re-inserts produce fresh ids; track them.
    pub current_merged: Vec<SectorId>,
    /// Sidedefs whose `.sector` was retargeted, with their original target.
    pub sidedef_changes: Vec<(SidedefId, SectorId /* old */)>,
    /// Linedefs to delete (only populated for "Merge Sectors" — they were
    /// shared between two merged sectors). Empty for plain "Join Sectors".
    pub removed_lines: Vec<(LinedefId, crate::map::MapLinedef)>,
    pub current_removed_lines: Vec<LinedefId>,
    /// Sidedefs cascaded along with `removed_lines`.
    pub removed_sides: Vec<(SidedefId, crate::map::MapSidedef)>,
    pub current_removed_sides: Vec<SidedefId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidedefSide {
    Right,
    Left,
}

/// Snapshot of selected map elements suitable for serialising to a
/// clipboard. Cross-references are by index into the snap vectors so the
/// data is portable across maps.
#[derive(Debug, Clone, Default)]
pub struct ClipboardData {
    pub vertices: Vec<crate::map::MapVertex>,
    pub sectors: Vec<crate::map::MapSector>,
    /// `(template, clip_sector_idx)` — the template's `sector` field is
    /// ignored at paste time; we substitute the resolved sector id.
    pub sidedefs: Vec<(crate::map::MapSidedef, usize)>,
    /// `(template, v1_idx, v2_idx, right_clip_idx, left_clip_idx)`.
    pub linedefs: Vec<(
        crate::map::MapLinedef,
        usize,
        usize,
        Option<usize>,
        Option<usize>,
    )>,
    pub things: Vec<crate::map::MapThing>,
}

#[derive(Debug, Clone, Default)]
pub struct PasteClipboardState {
    pub data: ClipboardData,
    /// World-space (dx, dy) added to every vertex and thing position.
    pub offset: (i32, i32),
    // Post-apply ids (cleared between apply/revert).
    pub current_v: Vec<VertexId>,
    pub current_sec: Vec<SectorId>,
    pub current_side: Vec<SidedefId>,
    pub current_line: Vec<LinedefId>,
    pub current_thing: Vec<ThingId>,
}

/// Old/new pair for a per-sidedef offset write. `None` in a slot leaves that
/// axis untouched.
#[derive(Debug, Clone, Copy)]
pub struct SidedefOffsetChange {
    pub id: SidedefId,
    pub old_x: i16,
    pub old_y: i16,
    pub new_x: Option<i16>,
    pub new_y: Option<i16>,
}

#[derive(Debug, Clone)]
pub struct SplitLine {
    pub line: LinedefId,
    /// v2 of the line before splitting; the split point becomes the new v2.
    pub original_v2: VertexId,
    pub new_v: Option<VertexId>,
    pub new_line: Option<LinedefId>,
    pub new_right: Option<SidedefId>,
    pub new_left: Option<SidedefId>,
    /// Explicit split coordinates. When `None`, the apply step splits at the
    /// linedef midpoint. Used by "insert vertex on line" to place the new
    /// vertex at the user's click position projected onto the line.
    pub override_pos: Option<(i32, i32)>,
}

#[derive(Debug, Clone, Default)]
pub struct SplitLinedefsState {
    pub splits: Vec<SplitLine>,
}

#[derive(Debug)]
pub enum SplitError {
    NoLines,
    LineMissing,
    VertexMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointSide {
    V1,
    V2,
}

#[derive(Debug)]
pub enum MergeError {
    NotEnoughVertices,
    VertexMissing,
}

#[derive(Debug, Clone)]
pub struct VertexMergeState {
    pub survivor: VertexId,
    pub survivor_old_pos: (i32, i32),
    pub survivor_new_pos: (i32, i32),
    /// Doomed vertices (excluded survivor), captured before mutation.
    pub removed_vertex_data: Vec<(VertexId, MapVertex)>,
    /// Linedefs that survive but had one endpoint redirected to the survivor.
    /// Stores (line, which side, the doomed vertex it used to point at).
    pub redirected: Vec<(LinedefId, EndpointSide, VertexId)>,
    /// Linedefs that became degenerate (v1 == v2 after redirect) and were
    /// removed entirely.
    pub removed_line_data: Vec<(LinedefId, MapLinedef)>,
    /// Sidedefs of removed linedefs.
    pub removed_side_data: Vec<(SidedefId, MapSidedef)>,
    /// After most recent revert: parallel lists of NEW slot-map ids assigned
    /// when the snapshots were re-inserted. Empty after a fresh apply.
    pub current_v: Vec<VertexId>,
    pub current_l: Vec<LinedefId>,
    pub current_s: Vec<SidedefId>,
}

/// Compute the merge state without mutating the map.
pub fn compute_vertex_merge(
    map: &Map,
    vertex_ids: &[VertexId],
) -> Result<VertexMergeState, MergeError> {
    if vertex_ids.len() < 2 {
        return Err(MergeError::NotEnoughVertices);
    }
    for v in vertex_ids {
        if !map.vertices.contains_key(*v) {
            return Err(MergeError::VertexMissing);
        }
    }

    let survivor = vertex_ids[0];
    let removed: HashSet<VertexId> = vertex_ids.iter().skip(1).copied().collect();

    // Centroid of all selected vertices.
    let mut sum_x = 0_i64;
    let mut sum_y = 0_i64;
    for v in vertex_ids {
        let p = &map.vertices[*v];
        sum_x += p.x as i64;
        sum_y += p.y as i64;
    }
    let n = vertex_ids.len() as i64;
    let new_pos = ((sum_x / n) as i32, (sum_y / n) as i32);
    let old_pos = {
        let p = &map.vertices[survivor];
        (p.x, p.y)
    };

    let removed_vertex_data: Vec<(VertexId, MapVertex)> = removed
        .iter()
        .map(|vid| (*vid, *map.vertices.get(*vid).unwrap()))
        .collect();

    let mut redirected: Vec<(LinedefId, EndpointSide, VertexId)> = Vec::new();
    let mut removed_line_data: Vec<(LinedefId, MapLinedef)> = Vec::new();
    let mut removed_side_data: Vec<(SidedefId, MapSidedef)> = Vec::new();

    for (lid, line) in &map.linedefs {
        let v1_doomed = removed.contains(&line.v1);
        let v2_doomed = removed.contains(&line.v2);
        if !v1_doomed && !v2_doomed {
            continue;
        }
        let post_v1 = if v1_doomed { survivor } else { line.v1 };
        let post_v2 = if v2_doomed { survivor } else { line.v2 };
        if post_v1 == post_v2 {
            removed_line_data.push((lid, line.clone()));
            if let Some(sid) = line.right {
                if let Some(s) = map.sidedefs.get(sid) {
                    removed_side_data.push((sid, s.clone()));
                }
            }
            if let Some(sid) = line.left {
                if let Some(s) = map.sidedefs.get(sid) {
                    removed_side_data.push((sid, s.clone()));
                }
            }
        } else {
            if v1_doomed {
                redirected.push((lid, EndpointSide::V1, line.v1));
            }
            if v2_doomed {
                redirected.push((lid, EndpointSide::V2, line.v2));
            }
        }
    }

    Ok(VertexMergeState {
        survivor,
        survivor_old_pos: old_pos,
        survivor_new_pos: new_pos,
        removed_vertex_data,
        redirected,
        removed_line_data,
        removed_side_data,
        current_v: Vec::new(),
        current_l: Vec::new(),
        current_s: Vec::new(),
    })
}

/// Build a [`SplitLinedefsState`] for the given linedefs. Doesn't mutate the
/// map; caller wraps in `Command::SplitLinedefs` and applies it.
pub fn compute_split_lines(
    map: &Map,
    line_ids: &[LinedefId],
) -> Result<SplitLinedefsState, SplitError> {
    if line_ids.is_empty() {
        return Err(SplitError::NoLines);
    }
    let mut splits = Vec::with_capacity(line_ids.len());
    for lid in line_ids {
        let line = map.linedefs.get(*lid).ok_or(SplitError::LineMissing)?;
        if !map.vertices.contains_key(line.v1) || !map.vertices.contains_key(line.v2) {
            return Err(SplitError::VertexMissing);
        }
        splits.push(SplitLine {
            line: *lid,
            original_v2: line.v2,
            new_v: None,
            new_line: None,
            new_right: None,
            new_left: None,
            override_pos: None,
        });
    }
    Ok(SplitLinedefsState { splits })
}

/// Build a one-element [`SplitLinedefsState`] that inserts a vertex on `line`
/// at the point on the segment nearest to `(wx, wy)`. Caller wraps in
/// `Command::SplitLinedefs` and applies it.
pub fn compute_insert_vertex_on_line(
    map: &Map,
    line: LinedefId,
    wx: f32,
    wy: f32,
) -> Result<SplitLinedefsState, SplitError> {
    let l = map.linedefs.get(line).ok_or(SplitError::LineMissing)?;
    let v1 = map.vertices.get(l.v1).ok_or(SplitError::VertexMissing)?;
    let v2 = map.vertices.get(l.v2).ok_or(SplitError::VertexMissing)?;
    // Project (wx, wy) onto segment v1..v2, clamped to (0, 1) so a near-miss
    // outside the segment still lands on the line itself.
    let dx = (v2.x - v1.x) as f32;
    let dy = (v2.y - v1.y) as f32;
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-6 {
        0.5
    } else {
        let tt = ((wx - v1.x as f32) * dx + (wy - v1.y as f32) * dy) / len2;
        // Bias the clamp inward by a hair so we never produce a zero-length
        // half-line on either side.
        tt.clamp(0.05, 0.95)
    };
    let px = (v1.x as f32 + t * dx).round() as i32;
    let py = (v1.y as f32 + t * dy).round() as i32;
    let splits = vec![SplitLine {
        line,
        original_v2: l.v2,
        new_v: None,
        new_line: None,
        new_right: None,
        new_left: None,
        override_pos: Some((px, py)),
    }];
    Ok(SplitLinedefsState { splits })
}

#[derive(Debug)]
pub enum MakeSectorError {
    NoLines,
    LineHasSides,
    NotAClosedLoop,
    DanglingVertex,
}

#[derive(Debug, Clone)]
pub struct MakeSectorState {
    pub sector_template: MapSector,
    pub sidedef_template: MapSidedef,
    pub line_assignments: Vec<(LinedefId, SidedefSide)>,
    pub current_sec: Option<SectorId>,
    pub current_sides: Vec<SidedefId>,
}

/// Compute a `MakeSectorState` from a closed loop of side-less linedefs.
/// Caller wraps it in a `Command::MakeSector` and applies it to mutate the map.
pub fn compute_make_sector(
    map: &Map,
    line_ids: &[LinedefId],
) -> Result<MakeSectorState, MakeSectorError> {
    if line_ids.is_empty() {
        return Err(MakeSectorError::NoLines);
    }
    // 1. Validate: linedefs exist and have no sides yet.
    for lid in line_ids {
        let l = map
            .linedefs
            .get(*lid)
            .ok_or(MakeSectorError::NotAClosedLoop)?;
        if l.right.is_some() || l.left.is_some() {
            return Err(MakeSectorError::LineHasSides);
        }
    }

    // 2. Build vertex adjacency over the selected subgraph.
    let mut adj: HashMap<VertexId, Vec<(VertexId, LinedefId)>> = HashMap::new();
    for lid in line_ids {
        let l = &map.linedefs[*lid];
        adj.entry(l.v1).or_default().push((l.v2, *lid));
        adj.entry(l.v2).or_default().push((l.v1, *lid));
    }
    if adj.values().any(|v| v.len() != 2) {
        return Err(MakeSectorError::DanglingVertex);
    }

    // 3. Walk the loop starting from the first linedef.
    let first = &map.linedefs[line_ids[0]];
    let start_v = first.v1;
    let mut current_v = first.v2;
    let mut walked: Vec<(VertexId, VertexId, LinedefId)> = vec![(start_v, current_v, line_ids[0])];
    let mut visited: HashSet<LinedefId> = [line_ids[0]].into_iter().collect();
    let max_iters = line_ids.len() + 1;
    let mut iter = 0;
    while current_v != start_v {
        iter += 1;
        if iter > max_iters {
            return Err(MakeSectorError::NotAClosedLoop);
        }
        let next = adj
            .get(&current_v)
            .and_then(|cands| cands.iter().find(|(_, lid)| !visited.contains(lid)));
        let Some(&(next_v, next_lid)) = next else {
            return Err(MakeSectorError::NotAClosedLoop);
        };
        walked.push((current_v, next_v, next_lid));
        visited.insert(next_lid);
        current_v = next_v;
    }
    if visited.len() != line_ids.len() {
        return Err(MakeSectorError::NotAClosedLoop);
    }

    // 4. Centroid of the loop.
    let mut cx = 0.0_f32;
    let mut cy = 0.0_f32;
    for (_, to, _) in &walked {
        let v = &map.vertices[*to];
        cx += v.x as f32;
        cy += v.y as f32;
    }
    cx /= walked.len() as f32;
    cy /= walked.len() as f32;

    // 5. Per-edge: decide which side the centroid lies on.
    let mut line_assignments: Vec<(LinedefId, SidedefSide)> = Vec::with_capacity(walked.len());
    for (from, to, lid) in &walked {
        let pa = &map.vertices[*from];
        let pb = &map.vertices[*to];
        let dx = (pb.x - pa.x) as f32;
        let dy = (pb.y - pa.y) as f32;
        let mid_x = (pa.x as f32 + pb.x as f32) * 0.5;
        let mid_y = (pa.y as f32 + pb.y as f32) * 0.5;
        let to_c_x = cx - mid_x;
        let to_c_y = cy - mid_y;
        // Left perpendicular (rotate 90° CCW in standard math axes).
        let perp_x = -dy;
        let perp_y = dx;
        let inside_is_walk_left = perp_x * to_c_x + perp_y * to_c_y > 0.0;

        let line = &map.linedefs[*lid];
        let walk_matches = line.v1 == *from && line.v2 == *to;
        // For Doom, "right" sidedef is on the right hand side as you walk
        // along the linedef from v1 to v2.
        let inside_is_linedef_right = if walk_matches {
            !inside_is_walk_left
        } else {
            inside_is_walk_left
        };
        let side = if inside_is_linedef_right {
            SidedefSide::Right
        } else {
            SidedefSide::Left
        };
        line_assignments.push((*lid, side));
    }

    // 6. Templates with sane defaults. Caller's command::apply does the insert.
    let sector_template = MapSector {
        floor_height: 0,
        ceiling_height: 128,
        floor_texture: TextureName(*b"FLOOR5_4"),
        ceiling_texture: TextureName(*b"CEIL3_5\0"),
        light: 160,
        special: 0,
        tag: 0,
        sidedefs: Vec::new(),
        fields: Default::default(),
    };
    let sidedef_template = MapSidedef {
        sector: SectorId::default(), // filled in by apply
        x_offset: 0,
        y_offset: 0,
        upper_texture: TextureName([0; 8]),
        lower_texture: TextureName([0; 8]),
        middle_texture: TextureName(*b"STARTAN3"),
    };

    Ok(MakeSectorState {
        sector_template,
        sidedef_template,
        line_assignments,
        current_sec: None,
        current_sides: Vec::new(),
    })
}

#[derive(Debug, Clone)]
pub enum LineEndpoint {
    /// References a vertex that existed BEFORE the chain (and still does after
    /// the chain's vertices have been removed via undo).
    Existing(VertexId),
    /// Index into `LinedefChain::vertex_inserts`.
    New(usize),
}

#[derive(Debug, Clone, Default)]
pub struct LinedefChain {
    pub vertex_inserts: Vec<MapVertex>,
    /// Per linedef: (from_endpoint, to_endpoint, template).
    pub linedefs: Vec<(LineEndpoint, LineEndpoint, MapLinedef)>,
    pub current_v: Vec<VertexId>,
    pub current_l: Vec<LinedefId>,
}

#[derive(Debug, Clone, Default)]
pub struct DeletionState {
    pub vertex_snaps: Vec<(VertexId, MapVertex)>,
    pub sector_snaps: Vec<(SectorId, MapSector)>,
    pub sidedef_snaps: Vec<(SidedefId, MapSidedef)>,
    pub linedef_snaps: Vec<(LinedefId, MapLinedef)>,
    pub thing_snaps: Vec<MapThing>,
    pub current_v: Vec<VertexId>,
    pub current_sec: Vec<SectorId>,
    pub current_side: Vec<SidedefId>,
    pub current_line: Vec<LinedefId>,
    pub current_thing: Vec<ThingId>,
}

/// Delete a selection (vertices / linedefs / sectors / things) with full
/// topology cleanup, returning the snapshot needed to undo it. Must be called
/// **before** the caller pushes a `Command::DeleteElements`.
pub fn collect_and_delete(
    map: &mut Map,
    selected_vertices: &HashSet<VertexId>,
    selected_linedefs: &HashSet<LinedefId>,
    selected_sectors: &HashSet<SectorId>,
    selected_things: &HashSet<ThingId>,
) -> DeletionState {
    let to_del_v = selected_vertices.clone();
    let mut to_del_l = selected_linedefs.clone();
    let to_del_sec = selected_sectors.clone();
    let mut to_del_side: HashSet<SidedefId> = HashSet::new();
    let to_del_t = selected_things.clone();

    // Linedefs touching deleted vertices.
    for (lid, line) in &map.linedefs {
        if to_del_v.contains(&line.v1) || to_del_v.contains(&line.v2) {
            to_del_l.insert(lid);
        }
    }
    // Sidedefs pointing at deleted sectors.
    for (sid, side) in &map.sidedefs {
        if to_del_sec.contains(&side.sector) {
            to_del_side.insert(sid);
        }
    }
    // Iterate: any linedef left without sidedefs goes; sidedefs of any
    // newly-marked linedef go too.
    loop {
        let mut changed = false;
        let lids: Vec<LinedefId> = to_del_l.iter().copied().collect();
        for lid in lids {
            if let Some(line) = map.linedefs.get(lid) {
                if let Some(r) = line.right {
                    if to_del_side.insert(r) {
                        changed = true;
                    }
                }
                if let Some(l) = line.left {
                    if to_del_side.insert(l) {
                        changed = true;
                    }
                }
            }
        }
        for (lid, line) in &map.linedefs {
            if to_del_l.contains(&lid) {
                continue;
            }
            let r_gone = line.right.map(|s| to_del_side.contains(&s)).unwrap_or(true);
            let l_gone = line.left.map(|s| to_del_side.contains(&s)).unwrap_or(true);
            if r_gone && l_gone {
                if to_del_l.insert(lid) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut state = DeletionState::default();
    for tid in &to_del_t {
        if let Some(t) = map.things.remove(*tid) {
            state.thing_snaps.push(t);
        }
    }
    for lid in &to_del_l {
        if let Some(l) = map.linedefs.remove(*lid) {
            state.linedef_snaps.push((*lid, l));
        }
    }
    for sid in &to_del_side {
        if let Some(s) = map.sidedefs.remove(*sid) {
            state.sidedef_snaps.push((*sid, s));
        }
    }
    for sid in &to_del_sec {
        if let Some(s) = map.sectors.remove(*sid) {
            state.sector_snaps.push((*sid, s));
        }
    }
    for vid in &to_del_v {
        if let Some(v) = map.vertices.remove(*vid) {
            state.vertex_snaps.push((*vid, v));
        }
    }
    state
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectorIntField {
    FloorHeight,
    CeilingHeight,
    Light,
    Tag,
    Special,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinedefIntField {
    Flags,
    Tag,
    Arg0,
    Arg1,
    Arg2,
    Arg3,
    Arg4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThingIntField {
    Angle,
    Flags,
}

#[derive(Debug)]
pub enum JoinError {
    NoSectors,
    SectorMissing,
}

/// Build a `JoinSectorsState` for absorbing `sectors` into the first id of
/// the list. With `remove_shared_lines = true`, linedefs whose both sides
/// pointed at sectors in the selection are deleted (and their sidedefs
/// cascade); otherwise only the sidedef.sector pointer is retargeted.
pub fn compute_join_sectors(
    map: &Map,
    sectors: &[SectorId],
    remove_shared_lines: bool,
) -> Result<JoinSectorsState, JoinError> {
    if sectors.is_empty() {
        return Err(JoinError::NoSectors);
    }
    for sid in sectors {
        if !map.sectors.contains_key(*sid) {
            return Err(JoinError::SectorMissing);
        }
    }
    let survivor = sectors[0];
    let merged_set: HashSet<SectorId> = sectors.iter().skip(1).copied().collect();
    if merged_set.is_empty() {
        // One sector selected — nothing to absorb.
        return Err(JoinError::NoSectors);
    }

    let merged_snapshots: Vec<(SectorId, crate::map::MapSector)> = merged_set
        .iter()
        .map(|sid| (*sid, map.sectors[*sid].clone()))
        .collect();

    // Sidedefs whose sector is one of the absorbed ones get retargeted.
    let mut sidedef_changes: Vec<(SidedefId, SectorId)> = Vec::new();
    for (sid, side) in &map.sidedefs {
        if merged_set.contains(&side.sector) {
            sidedef_changes.push((sid, side.sector));
        }
    }

    let mut removed_lines: Vec<(LinedefId, crate::map::MapLinedef)> = Vec::new();
    let mut removed_sides: Vec<(SidedefId, crate::map::MapSidedef)> = Vec::new();
    if remove_shared_lines {
        // Combined set of all sectors involved (survivor + absorbed).
        let involved: HashSet<SectorId> =
            std::iter::once(survivor).chain(merged_set.iter().copied()).collect();
        let mut removed_side_ids: HashSet<SidedefId> = HashSet::new();
        for (lid, line) in &map.linedefs {
            let (Some(r), Some(l)) = (line.right, line.left) else {
                continue;
            };
            let (Some(rs), Some(ls)) =
                (map.sidedefs.get(r), map.sidedefs.get(l))
            else {
                continue;
            };
            if involved.contains(&rs.sector) && involved.contains(&ls.sector) {
                removed_lines.push((lid, line.clone()));
                removed_side_ids.insert(r);
                removed_side_ids.insert(l);
            }
        }
        for sid in removed_side_ids {
            if let Some(s) = map.sidedefs.get(sid) {
                removed_sides.push((sid, s.clone()));
            }
        }
        // Drop sidedef_changes for sides we're already going to remove.
        let removed_set: HashSet<SidedefId> =
            removed_sides.iter().map(|(id, _)| *id).collect();
        sidedef_changes.retain(|(sid, _)| !removed_set.contains(sid));
    }

    Ok(JoinSectorsState {
        survivor,
        merged_snapshots,
        current_merged: Vec::new(),
        sidedef_changes,
        removed_lines,
        current_removed_lines: Vec::new(),
        removed_sides,
        current_removed_sides: Vec::new(),
    })
}

/// Find pairs of overlapping linedefs within `line_ids` and produce stitch
/// merges. Two linedefs "overlap" when they share their endpoint vertex
/// pair (regardless of v1/v2 order). For each opposite-direction pair, the
/// second is merged into the first by reassigning its right sidedef as the
/// first's left, then removing the second line.
///
/// Same-direction overlaps and pairs where neither line has a right sidedef
/// are skipped — there's nothing useful to stitch.
pub fn compute_stitch_lines(map: &Map, line_ids: &[LinedefId]) -> Vec<StitchMerge> {
    if line_ids.len() < 2 {
        return Vec::new();
    }
    let mut merges: Vec<StitchMerge> = Vec::new();
    let mut consumed: HashSet<LinedefId> = HashSet::new();
    // Group by unordered vertex pair.
    let mut groups: HashMap<(VertexId, VertexId), Vec<LinedefId>> = HashMap::new();
    for lid in line_ids {
        let Some(l) = map.linedefs.get(*lid) else { continue };
        let key = if l.v1 <= l.v2 { (l.v1, l.v2) } else { (l.v2, l.v1) };
        groups.entry(key).or_default().push(*lid);
    }
    for (_, lids) in groups.into_iter().filter(|(_, v)| v.len() >= 2) {
        // Sort by raw id for deterministic ordering between runs.
        let mut lids = lids;
        lids.sort();
        let keeper = lids[0];
        if consumed.contains(&keeper) {
            continue;
        }
        let Some(keeper_line) = map.linedefs.get(keeper) else { continue };
        let keeper_dir = (keeper_line.v1, keeper_line.v2);
        for &absorbed in &lids[1..] {
            if consumed.contains(&absorbed) {
                continue;
            }
            let Some(abs_line) = map.linedefs.get(absorbed) else { continue };
            // Opposite direction = absorbed.v1 == keeper.v2 && absorbed.v2 == keeper.v1.
            let opposite = abs_line.v1 == keeper_dir.1 && abs_line.v2 == keeper_dir.0;
            if !opposite {
                // Same direction; skip — no useful stitch.
                continue;
            }
            if abs_line.right.is_none() {
                continue;
            }
            // Keeper must currently lack a left side (otherwise we'd
            // overwrite an existing sidedef ref).
            if keeper_line.left.is_some() {
                continue;
            }
            let absorbed_left_snap = abs_line.left.and_then(|sid| {
                map.sidedefs.get(sid).map(|s| (sid, s.clone()))
            });
            merges.push(StitchMerge {
                keeper,
                keeper_old_left: keeper_line.left,
                keeper_new_left: abs_line.right,
                absorbed_line_id: absorbed,
                absorbed_line_snap: abs_line.clone(),
                absorbed_left_snap,
                current_absorbed_line: None,
                current_absorbed_left: None,
            });
            consumed.insert(keeper);
            consumed.insert(absorbed);
            break;
        }
    }
    merges
}

/// Pack a selection (any combination of vertex / linedef / sector / thing
/// ids) into a portable `ClipboardData`. Implicitly pulls in dependencies:
/// selected linedefs bring their endpoints; selected sectors bring their
/// boundary linedefs, endpoints, and sidedefs.
pub fn build_clipboard(
    map: &Map,
    sel_vertices: &HashSet<VertexId>,
    sel_lines: &HashSet<LinedefId>,
    sel_sectors: &HashSet<SectorId>,
    sel_things: &HashSet<ThingId>,
) -> ClipboardData {
    let mut data = ClipboardData::default();
    let mut v_index: HashMap<VertexId, usize> = HashMap::new();
    let mut s_index: HashMap<SectorId, usize> = HashMap::new();
    let mut side_index: HashMap<SidedefId, usize> = HashMap::new();
    let mut line_set: HashSet<LinedefId> = sel_lines.iter().copied().collect();

    // Expand: selected sectors pull in any linedef touching them.
    if !sel_sectors.is_empty() {
        for (lid, l) in &map.linedefs {
            let right_sec = l.right.and_then(|s| map.sidedefs.get(s).map(|x| x.sector));
            let left_sec = l.left.and_then(|s| map.sidedefs.get(s).map(|x| x.sector));
            if right_sec.map(|s| sel_sectors.contains(&s)).unwrap_or(false)
                || left_sec.map(|s| sel_sectors.contains(&s)).unwrap_or(false)
            {
                line_set.insert(lid);
            }
        }
    }

    // Vertex pool: explicit selection + every endpoint of a selected line.
    let mut vertex_pool: HashSet<VertexId> = sel_vertices.iter().copied().collect();
    for lid in &line_set {
        if let Some(l) = map.linedefs.get(*lid) {
            vertex_pool.insert(l.v1);
            vertex_pool.insert(l.v2);
        }
    }
    let mut vertex_order: Vec<VertexId> = vertex_pool.into_iter().collect();
    vertex_order.sort();
    for vid in &vertex_order {
        if let Some(v) = map.vertices.get(*vid) {
            v_index.insert(*vid, data.vertices.len());
            data.vertices.push(*v);
        }
    }

    // Sectors.
    let mut sector_order: Vec<SectorId> = sel_sectors.iter().copied().collect();
    sector_order.sort();
    for sid in &sector_order {
        if let Some(s) = map.sectors.get(*sid) {
            s_index.insert(*sid, data.sectors.len());
            data.sectors.push(s.clone());
        }
    }

    // Sidedefs: any used by a clipboard linedef whose sector is also in clip.
    let mut line_order: Vec<LinedefId> = line_set.iter().copied().collect();
    line_order.sort();
    for lid in &line_order {
        let Some(l) = map.linedefs.get(*lid) else { continue };
        for slot in [l.right, l.left].iter().copied().flatten() {
            if side_index.contains_key(&slot) {
                continue;
            }
            let Some(side) = map.sidedefs.get(slot) else { continue };
            let Some(&sec_idx) = s_index.get(&side.sector) else { continue };
            side_index.insert(slot, data.sidedefs.len());
            data.sidedefs.push((side.clone(), sec_idx));
        }
    }

    // Linedefs (now we can resolve all references).
    for lid in &line_order {
        let Some(l) = map.linedefs.get(*lid) else { continue };
        let (Some(&v1), Some(&v2)) = (v_index.get(&l.v1), v_index.get(&l.v2)) else {
            continue;
        };
        let right = l.right.and_then(|s| side_index.get(&s).copied());
        let left = l.left.and_then(|s| side_index.get(&s).copied());
        data.linedefs.push((l.clone(), v1, v2, right, left));
    }

    // Things.
    let mut thing_order: Vec<ThingId> = sel_things.iter().copied().collect();
    thing_order.sort();
    for tid in &thing_order {
        if let Some(t) = map.things.get(*tid) {
            data.things.push(t.clone());
        }
    }

    data
}

/// Order a set of linedefs into a single walk-path (sequence of vertex ids
/// and the linedef traversed between each adjacent pair). Returns `None` if
/// the selection isn't a single open or closed chain (any vertex shared by
/// more than two lines).
pub fn order_linedef_chain(
    map: &Map,
    line_ids: &[LinedefId],
) -> Option<Vec<(VertexId, LinedefId)>> {
    let mut adj: HashMap<VertexId, Vec<(VertexId, LinedefId)>> = HashMap::new();
    for lid in line_ids {
        let l = map.linedefs.get(*lid)?;
        adj.entry(l.v1).or_default().push((l.v2, *lid));
        adj.entry(l.v2).or_default().push((l.v1, *lid));
    }
    if adj.values().any(|v| v.len() > 2) {
        return None;
    }
    // Pick a starting vertex: an endpoint (degree 1) for an open chain;
    // otherwise any vertex (closed loop).
    let start = adj
        .iter()
        .find(|(_, ns)| ns.len() == 1)
        .map(|(v, _)| *v)
        .or_else(|| adj.keys().next().copied())?;
    let mut path: Vec<(VertexId, LinedefId)> = Vec::new();
    let mut visited_lines: HashSet<LinedefId> = HashSet::new();
    let mut cur = start;
    loop {
        let next = adj
            .get(&cur)?
            .iter()
            .find(|(_, lid)| !visited_lines.contains(lid))
            .copied();
        let Some((nxt_v, nxt_l)) = next else { break };
        visited_lines.insert(nxt_l);
        path.push((cur, nxt_l));
        cur = nxt_v;
        if visited_lines.len() == line_ids.len() {
            path.push((cur, LinedefId::default())); // sentinel for the terminal vertex
            break;
        }
    }
    if visited_lines.len() != line_ids.len() {
        return None;
    }
    Some(path)
}

/// Build a `Command::FlipLinedefs` that re-orients selected lines so each
/// line's right (front) sidedef points outward from the loop's centroid.
/// Lines without two endpoints are skipped. Works on any selection — even
/// when they don't form a single closed chain — by treating each line
/// independently against the *selection's* centroid.
pub fn compute_align_linedefs(map: &Map, line_ids: &[LinedefId]) -> Vec<LinedefId> {
    if line_ids.is_empty() {
        return Vec::new();
    }
    // Centroid of all vertex endpoints in the selection.
    let mut cx = 0.0_f32;
    let mut cy = 0.0_f32;
    let mut count = 0.0_f32;
    for lid in line_ids {
        if let Some(l) = map.linedefs.get(*lid) {
            if let (Some(a), Some(b)) = (map.vertices.get(l.v1), map.vertices.get(l.v2)) {
                cx += a.x as f32 + b.x as f32;
                cy += a.y as f32 + b.y as f32;
                count += 2.0;
            }
        }
    }
    if count < 1.0 {
        return Vec::new();
    }
    cx /= count;
    cy /= count;
    // For each line, the right-of-walk normal in math-Y-up is (dy, -dx).
    // If this normal points AWAY from the centroid (positive dot with the
    // mid→centroid vector flipped), the line is already aligned. Else flip.
    let mut to_flip: Vec<LinedefId> = Vec::new();
    for lid in line_ids {
        let Some(l) = map.linedefs.get(*lid) else { continue };
        let (Some(a), Some(b)) = (map.vertices.get(l.v1), map.vertices.get(l.v2)) else {
            continue;
        };
        let dx = (b.x - a.x) as f32;
        let dy = (b.y - a.y) as f32;
        // Right-side normal in math-Y-up coords.
        let nx = dy;
        let ny = -dx;
        let mx = (a.x as f32 + b.x as f32) * 0.5;
        let my = (a.y as f32 + b.y as f32) * 0.5;
        let to_outside_x = mx - cx;
        let to_outside_y = my - cy;
        // Dot < 0 means the right-side normal points *towards* the centroid
        // (i.e., front side faces inward) — flip to align with convention.
        let dot = nx * to_outside_x + ny * to_outside_y;
        if dot < 0.0 {
            to_flip.push(*lid);
        }
    }
    to_flip
}

#[derive(Debug, Clone, Copy)]
pub enum AutoAlignAxis {
    X,
    Y,
    Both,
}

/// Walk an ordered chain of linedefs (e.g. from `order_linedef_chain`) and
/// compute per-sidedef X-offset values so the wall texture flows seamlessly
/// across all selected lines. Picks the right-side sidedef of each line; if
/// only a left side exists, falls back to that. The first sidedef keeps its
/// current X offset; each subsequent one is set to `prev_x + prev_length`
/// modulo a reasonable texture width (256 is a safe pick when we don't have
/// access to the actual texture's pixel width).
///
/// For axis Y or Both, each sidedef's Y offset becomes the floor-height
/// delta relative to the chain's starting sector floor — matches how
/// vanilla Doom aligns vertical seams when a wall steps up or down.
pub fn compute_auto_align_textures(
    map: &Map,
    line_ids: &[LinedefId],
    axis: AutoAlignAxis,
) -> Vec<SidedefOffsetChange> {
    if line_ids.is_empty() {
        return Vec::new();
    }
    let Some(path) = order_linedef_chain(map, line_ids) else {
        return Vec::new();
    };
    let mut changes: Vec<SidedefOffsetChange> = Vec::new();
    let mut accumulated: f32 = 0.0;
    let mut base_floor: Option<i16> = None;
    for window_idx in 0..path.len().saturating_sub(1) {
        let (from_v, lid) = path[window_idx];
        let (to_v, _) = path[window_idx + 1];
        let Some(line) = map.linedefs.get(lid) else { continue };
        let sid = match (line.right, line.left) {
            (Some(r), _) => r,
            (None, Some(l)) => l,
            _ => continue,
        };
        let Some(side) = map.sidedefs.get(sid) else { continue };
        // First line in chain → seed the X with its current offset.
        if changes.is_empty() {
            accumulated = side.x_offset as f32;
        }
        let length = match (map.vertices.get(from_v), map.vertices.get(to_v)) {
            (Some(a), Some(b)) => {
                let dx = (b.x - a.x) as f32;
                let dy = (b.y - a.y) as f32;
                (dx * dx + dy * dy).sqrt()
            }
            _ => 0.0,
        };
        let new_x = match axis {
            AutoAlignAxis::X | AutoAlignAxis::Both => {
                Some(((accumulated.round() as i32) & 0xFFFF) as i16)
            }
            _ => None,
        };
        let new_y = match axis {
            AutoAlignAxis::Y | AutoAlignAxis::Both => {
                let floor_h = map.sectors.get(side.sector).map(|s| s.floor_height);
                match (base_floor, floor_h) {
                    (None, Some(h)) => {
                        base_floor = Some(h);
                        Some(side.y_offset)
                    }
                    (Some(base), Some(h)) => Some((h - base).clamp(i16::MIN, i16::MAX)),
                    _ => None,
                }
            }
            _ => None,
        };
        changes.push(SidedefOffsetChange {
            id: sid,
            old_x: side.x_offset,
            old_y: side.y_offset,
            new_x,
            new_y,
        });
        accumulated += length;
    }
    changes
}

impl Command {
    pub fn apply(&mut self, map: &mut Map) {
        match self {
            Command::MoveVertices(moves) => {
                for m in moves.iter() {
                    if let Some(v) = map.vertices.get_mut(m.id) {
                        v.x = v.x.saturating_add(m.dx);
                        v.y = v.y.saturating_add(m.dy);
                    }
                }
            }
            Command::MoveThings(moves) => {
                for m in moves.iter() {
                    if let Some(t) = map.things.get_mut(m.id) {
                        t.x = t.x.saturating_add(m.dx);
                        t.y = t.y.saturating_add(m.dy);
                    }
                }
            }
            Command::SetSidedefTexture { id, slot, new, .. } => {
                if let Some(side) = map.sidedefs.get_mut(*id) {
                    write_sidedef_slot(side, *slot, *new);
                }
            }
            Command::SetSectorTexture { id, slot, new, .. } => {
                if let Some(sec) = map.sectors.get_mut(*id) {
                    write_sector_slot(sec, *slot, *new);
                }
            }
            Command::CreateThing { id, snapshot } => {
                if id.is_none() {
                    *id = Some(map.things.insert(snapshot.clone()));
                }
            }
            Command::DeleteThings {
                snapshots: _,
                current_ids,
            } => {
                for tid in current_ids.drain(..) {
                    map.things.remove(tid);
                }
            }
            Command::SetLinedefSpecial { id, new, .. } => {
                if let Some(line) = map.linedefs.get_mut(*id) {
                    line.special = *new;
                }
            }
            Command::SetThingKind { id, new, .. } => {
                if let Some(t) = map.things.get_mut(*id) {
                    t.kind = *new;
                }
            }
            Command::DeleteElements(state) => {
                // Remove whatever we last inserted on revert.
                for id in state.current_thing.drain(..) {
                    map.things.remove(id);
                }
                for id in state.current_line.drain(..) {
                    map.linedefs.remove(id);
                }
                for id in state.current_side.drain(..) {
                    map.sidedefs.remove(id);
                }
                for id in state.current_sec.drain(..) {
                    map.sectors.remove(id);
                }
                for id in state.current_v.drain(..) {
                    map.vertices.remove(id);
                }
            }
            Command::MergeVertices(state) => {
                // Build remaps from snapshot (original) ids → current map ids.
                // After a fresh apply these are identity (data still has the
                // original ids). After undo+redo the current_* vectors hold
                // the new ids assigned during revert.
                let mut remap_v: HashMap<VertexId, VertexId> = HashMap::new();
                if state.current_v.len() == state.removed_vertex_data.len() {
                    for (i, (orig, _)) in state.removed_vertex_data.iter().enumerate() {
                        remap_v.insert(*orig, state.current_v[i]);
                    }
                }
                let mut remap_l: HashMap<LinedefId, LinedefId> = HashMap::new();
                if state.current_l.len() == state.removed_line_data.len() {
                    for (i, (orig, _)) in state.removed_line_data.iter().enumerate() {
                        remap_l.insert(*orig, state.current_l[i]);
                    }
                }

                // 1. Redirect surviving lines' endpoints to the survivor.
                for (lid, side, _orig_v) in &state.redirected {
                    let cur_lid = *remap_l.get(lid).unwrap_or(lid);
                    if let Some(line) = map.linedefs.get_mut(cur_lid) {
                        match side {
                            EndpointSide::V1 => line.v1 = state.survivor,
                            EndpointSide::V2 => line.v2 = state.survivor,
                        }
                    }
                }
                // 2. Remove sidedefs of degenerate lines.
                let cur_s_ids: Vec<SidedefId> = if state.current_s.len()
                    == state.removed_side_data.len()
                {
                    state.current_s.clone()
                } else {
                    state.removed_side_data.iter().map(|(id, _)| *id).collect()
                };
                for sid in &cur_s_ids {
                    map.sidedefs.remove(*sid);
                }
                // 3. Remove degenerate lines.
                let cur_l_ids: Vec<LinedefId> = if !remap_l.is_empty() {
                    state.current_l.clone()
                } else {
                    state.removed_line_data.iter().map(|(id, _)| *id).collect()
                };
                for lid in &cur_l_ids {
                    map.linedefs.remove(*lid);
                }
                // 4. Remove doomed vertices.
                let cur_v_ids: Vec<VertexId> = if !remap_v.is_empty() {
                    state.current_v.clone()
                } else {
                    state.removed_vertex_data.iter().map(|(id, _)| *id).collect()
                };
                for vid in &cur_v_ids {
                    map.vertices.remove(*vid);
                }
                state.current_v.clear();
                state.current_l.clear();
                state.current_s.clear();
                // 5. Move survivor to centroid.
                if let Some(s) = map.vertices.get_mut(state.survivor) {
                    s.x = state.survivor_new_pos.0;
                    s.y = state.survivor_new_pos.1;
                }
                map.rebuild_sidedef_index();
            }
            Command::SplitLinedefs(state) => {
                for split in &mut state.splits {
                    let Some(orig) = map.linedefs.get(split.line).cloned() else {
                        continue;
                    };
                    let v1 = match map.vertices.get(orig.v1) {
                        Some(v) => *v,
                        None => continue,
                    };
                    let v2 = match map.vertices.get(split.original_v2) {
                        Some(v) => *v,
                        None => continue,
                    };
                    let mid = match split.override_pos {
                        Some((x, y)) => MapVertex { x, y },
                        None => MapVertex {
                            x: (v1.x + v2.x) / 2,
                            y: (v1.y + v2.y) / 2,
                        },
                    };
                    let mid_id = map.vertices.insert(mid);
                    split.new_v = Some(mid_id);

                    // Clone sidedefs (each half keeps its own per-line offsets).
                    let new_right = orig.right.and_then(|sid| {
                        let s = map.sidedefs.get(sid)?.clone();
                        Some(map.sidedefs.insert(s))
                    });
                    let new_left = orig.left.and_then(|sid| {
                        let s = map.sidedefs.get(sid)?.clone();
                        Some(map.sidedefs.insert(s))
                    });
                    split.new_right = new_right;
                    split.new_left = new_left;

                    let new_line = MapLinedef {
                        v1: mid_id,
                        v2: split.original_v2,
                        flags: orig.flags,
                        special: orig.special,
                        args: orig.args,
                        tag: orig.tag,
                        right: new_right,
                        left: new_left,
                        fields: orig.fields.clone(),
                    };
                    let new_lid = map.linedefs.insert(new_line);
                    split.new_line = Some(new_lid);

                    // Truncate the original.
                    if let Some(line) = map.linedefs.get_mut(split.line) {
                        line.v2 = mid_id;
                    }
                }
                map.rebuild_sidedef_index();
            }
            Command::MakeSector(state) => {
                let new_sec = map.sectors.insert(state.sector_template.clone());
                state.current_sec = Some(new_sec);
                state.current_sides.clear();
                for (lid, side) in &state.line_assignments {
                    let mut tmpl = state.sidedef_template.clone();
                    tmpl.sector = new_sec;
                    let sid = map.sidedefs.insert(tmpl);
                    state.current_sides.push(sid);
                    if let Some(line) = map.linedefs.get_mut(*lid) {
                        match side {
                            SidedefSide::Right => line.right = Some(sid),
                            SidedefSide::Left => line.left = Some(sid),
                        }
                    }
                }
                map.rebuild_sidedef_index();
            }
            Command::CreateLinedefChain(chain) => {
                // Re-create everything from snapshots; rebuild current_*.
                chain.current_v.clear();
                chain.current_l.clear();
                let mut new_v_ids: Vec<VertexId> = Vec::with_capacity(chain.vertex_inserts.len());
                for v in &chain.vertex_inserts {
                    let id = map.vertices.insert(*v);
                    new_v_ids.push(id);
                    chain.current_v.push(id);
                }
                for (a, b, line) in &chain.linedefs {
                    let mut new_line = line.clone();
                    new_line.v1 = match a {
                        LineEndpoint::Existing(id) => *id,
                        LineEndpoint::New(idx) => new_v_ids[*idx],
                    };
                    new_line.v2 = match b {
                        LineEndpoint::Existing(id) => *id,
                        LineEndpoint::New(idx) => new_v_ids[*idx],
                    };
                    let id = map.linedefs.insert(new_line);
                    chain.current_l.push(id);
                }
            }
            Command::SetSectorIntField { id, field, new, .. } => {
                if let Some(s) = map.sectors.get_mut(*id) {
                    write_sector_int(s, *field, *new);
                }
            }
            Command::SetLinedefIntField { id, field, new, .. } => {
                if let Some(l) = map.linedefs.get_mut(*id) {
                    write_linedef_int(l, *field, *new);
                }
            }
            Command::SetThingIntField { id, field, new, .. } => {
                if let Some(t) = map.things.get_mut(*id) {
                    write_thing_int(t, *field, *new);
                }
            }
            Command::FlipLinedefs(ids) => {
                for id in ids.iter() {
                    if let Some(line) = map.linedefs.get_mut(*id) {
                        std::mem::swap(&mut line.v1, &mut line.v2);
                        std::mem::swap(&mut line.right, &mut line.left);
                    }
                }
            }
            Command::FlipSidedefs(ids) => {
                for id in ids.iter() {
                    if let Some(line) = map.linedefs.get_mut(*id) {
                        std::mem::swap(&mut line.right, &mut line.left);
                    }
                }
            }
            Command::SetSidedefOffsets(changes) => {
                for c in changes.iter() {
                    if let Some(side) = map.sidedefs.get_mut(c.id) {
                        if let Some(x) = c.new_x {
                            side.x_offset = x;
                        }
                        if let Some(y) = c.new_y {
                            side.y_offset = y;
                        }
                    }
                }
            }
            Command::StitchLines(merges) => {
                for m in merges.iter_mut() {
                    // 1. Detach absorbed.right so deletion doesn't cascade
                    //    the sidedef we're about to reassign.
                    let absorbed_right = if let Some(line) = map.linedefs.get_mut(m.absorbed_line_id)
                    {
                        let r = line.right;
                        line.right = None;
                        r
                    } else {
                        None
                    };
                    // 2. Reassign that sidedef onto keeper.left.
                    if let Some(line) = map.linedefs.get_mut(m.keeper) {
                        m.keeper_old_left = line.left;
                        line.left = absorbed_right;
                        m.keeper_new_left = absorbed_right;
                    }
                    // 3. Remove the absorbed line's leftover left sidedef
                    //    (if any) and the line itself.
                    if let Some(left_id) = m.absorbed_left_snap.as_ref().map(|(id, _)| *id) {
                        map.sidedefs.remove(left_id);
                    }
                    map.linedefs.remove(m.absorbed_line_id);
                    m.current_absorbed_line = None;
                    m.current_absorbed_left = None;
                }
                map.rebuild_sidedef_index();
            }
            Command::PasteClipboard(state) => {
                state.current_v.clear();
                state.current_sec.clear();
                state.current_side.clear();
                state.current_line.clear();
                state.current_thing.clear();
                let (dx, dy) = state.offset;
                // 1. Vertices.
                for v in &state.data.vertices {
                    let id = map.vertices.insert(crate::map::MapVertex {
                        x: v.x.saturating_add(dx),
                        y: v.y.saturating_add(dy),
                    });
                    state.current_v.push(id);
                }
                // 2. Sectors.
                for s in &state.data.sectors {
                    let mut snap = s.clone();
                    snap.sidedefs.clear(); // rebuilt by rebuild_sidedef_index
                    let id = map.sectors.insert(snap);
                    state.current_sec.push(id);
                }
                // 3. Sidedefs (resolve sector index).
                for (template, sec_idx) in &state.data.sidedefs {
                    let mut snap = template.clone();
                    if let Some(&new_sec) = state.current_sec.get(*sec_idx) {
                        snap.sector = new_sec;
                    }
                    let id = map.sidedefs.insert(snap);
                    state.current_side.push(id);
                }
                // 4. Linedefs (resolve vertex + sidedef indices).
                for (template, v1, v2, right, left) in &state.data.linedefs {
                    let mut snap = template.clone();
                    if let Some(&new_v1) = state.current_v.get(*v1) {
                        snap.v1 = new_v1;
                    }
                    if let Some(&new_v2) = state.current_v.get(*v2) {
                        snap.v2 = new_v2;
                    }
                    snap.right = right.and_then(|i| state.current_side.get(i).copied());
                    snap.left = left.and_then(|i| state.current_side.get(i).copied());
                    let id = map.linedefs.insert(snap);
                    state.current_line.push(id);
                }
                // 5. Things.
                for t in &state.data.things {
                    let mut snap = t.clone();
                    snap.x = snap.x.saturating_add(dx);
                    snap.y = snap.y.saturating_add(dy);
                    let id = map.things.insert(snap);
                    state.current_thing.push(id);
                }
                map.rebuild_sidedef_index();
            }
            Command::Batch(cmds) => {
                for cmd in cmds.iter_mut() {
                    cmd.apply(map);
                }
            }
            Command::JoinSectors(state) => {
                // Build remaps from snapshot (original) sector ids → current.
                let mut sec_remap: HashMap<SectorId, SectorId> = HashMap::new();
                if state.current_merged.len() == state.merged_snapshots.len() {
                    for (i, (orig, _)) in state.merged_snapshots.iter().enumerate() {
                        sec_remap.insert(*orig, state.current_merged[i]);
                    }
                }
                // 1. Retarget sidedefs to the survivor.
                for (sid, _old) in &state.sidedef_changes {
                    if let Some(side) = map.sidedefs.get_mut(*sid) {
                        side.sector = state.survivor;
                    }
                }
                // 2. Remove sidedefs cascaded along with shared linedefs.
                state.current_removed_sides.clear();
                let sides_to_remove: Vec<SidedefId> = state
                    .removed_sides
                    .iter()
                    .map(|(id, _)| *id)
                    .collect();
                for sid in sides_to_remove {
                    map.sidedefs.remove(sid);
                }
                // 3. Remove shared linedefs.
                state.current_removed_lines.clear();
                let lines_to_remove: Vec<LinedefId> = state
                    .removed_lines
                    .iter()
                    .map(|(id, _)| *id)
                    .collect();
                for lid in lines_to_remove {
                    map.linedefs.remove(lid);
                }
                // 4. Remove the absorbed sectors themselves.
                state.current_merged.clear();
                let secs_to_remove: Vec<SectorId> = state
                    .merged_snapshots
                    .iter()
                    .map(|(id, _)| *id)
                    .collect();
                for sid in secs_to_remove {
                    map.sectors.remove(sid);
                }
                map.rebuild_sidedef_index();
            }
        }
    }

    pub fn revert(&mut self, map: &mut Map) {
        match self {
            Command::MoveVertices(moves) => {
                for m in moves.iter() {
                    if let Some(v) = map.vertices.get_mut(m.id) {
                        v.x = v.x.saturating_sub(m.dx);
                        v.y = v.y.saturating_sub(m.dy);
                    }
                }
            }
            Command::MoveThings(moves) => {
                for m in moves.iter() {
                    if let Some(t) = map.things.get_mut(m.id) {
                        t.x = t.x.saturating_sub(m.dx);
                        t.y = t.y.saturating_sub(m.dy);
                    }
                }
            }
            Command::SetSidedefTexture { id, slot, old, .. } => {
                if let Some(side) = map.sidedefs.get_mut(*id) {
                    write_sidedef_slot(side, *slot, *old);
                }
            }
            Command::SetSectorTexture { id, slot, old, .. } => {
                if let Some(sec) = map.sectors.get_mut(*id) {
                    write_sector_slot(sec, *slot, *old);
                }
            }
            Command::CreateThing { id, .. } => {
                if let Some(tid) = id.take() {
                    map.things.remove(tid);
                }
            }
            Command::DeleteThings {
                snapshots,
                current_ids,
            } => {
                current_ids.clear();
                for snap in snapshots.iter() {
                    let tid = map.things.insert(snap.clone());
                    current_ids.push(tid);
                }
            }
            Command::SetLinedefSpecial { id, old, .. } => {
                if let Some(line) = map.linedefs.get_mut(*id) {
                    line.special = *old;
                }
            }
            Command::SetThingKind { id, old, .. } => {
                if let Some(t) = map.things.get_mut(*id) {
                    t.kind = *old;
                }
            }
            Command::CreateLinedefChain(chain) => {
                for id in chain.current_l.drain(..) {
                    map.linedefs.remove(id);
                }
                for id in chain.current_v.drain(..) {
                    map.vertices.remove(id);
                }
            }
            Command::MakeSector(state) => {
                for (lid, side) in &state.line_assignments {
                    if let Some(line) = map.linedefs.get_mut(*lid) {
                        match side {
                            SidedefSide::Right => line.right = None,
                            SidedefSide::Left => line.left = None,
                        }
                    }
                }
                for sid in state.current_sides.drain(..) {
                    map.sidedefs.remove(sid);
                }
                if let Some(sec) = state.current_sec.take() {
                    map.sectors.remove(sec);
                }
                map.rebuild_sidedef_index();
            }
            Command::MergeVertices(state) => {
                // 1. Re-insert removed vertices, build remap.
                let mut remap_v: HashMap<VertexId, VertexId> = HashMap::new();
                state.current_v.clear();
                for (orig_id, v) in &state.removed_vertex_data {
                    let new_id = map.vertices.insert(*v);
                    remap_v.insert(*orig_id, new_id);
                    state.current_v.push(new_id);
                }
                // 2. Re-insert removed sidedefs (sectors weren't deleted, so
                //    the sector ref in each MapSidedef is still valid).
                let mut remap_s: HashMap<SidedefId, SidedefId> = HashMap::new();
                state.current_s.clear();
                for (orig_id, s) in &state.removed_side_data {
                    let new_id = map.sidedefs.insert(s.clone());
                    remap_s.insert(*orig_id, new_id);
                    state.current_s.push(new_id);
                }
                // 3. Re-insert removed lines, patching v1/v2 + sidedef refs.
                state.current_l.clear();
                for (_orig_id, l) in &state.removed_line_data {
                    let mut nl = l.clone();
                    if let Some(nv) = remap_v.get(&nl.v1) {
                        nl.v1 = *nv;
                    }
                    if let Some(nv) = remap_v.get(&nl.v2) {
                        nl.v2 = *nv;
                    }
                    nl.right = nl.right.and_then(|s| remap_s.get(&s).copied());
                    nl.left = nl.left.and_then(|s| remap_s.get(&s).copied());
                    let new_id = map.linedefs.insert(nl);
                    state.current_l.push(new_id);
                }
                // 4. Patch redirected linedefs back to their original endpoint.
                for (lid, side, orig_v) in &state.redirected {
                    let new_v = *remap_v.get(orig_v).unwrap_or(orig_v);
                    if let Some(line) = map.linedefs.get_mut(*lid) {
                        match side {
                            EndpointSide::V1 => line.v1 = new_v,
                            EndpointSide::V2 => line.v2 = new_v,
                        }
                    }
                }
                // 5. Restore survivor's old position.
                if let Some(s) = map.vertices.get_mut(state.survivor) {
                    s.x = state.survivor_old_pos.0;
                    s.y = state.survivor_old_pos.1;
                }
                map.rebuild_sidedef_index();
            }
            Command::SplitLinedefs(state) => {
                for split in state.splits.iter_mut().rev() {
                    if let Some(line) = map.linedefs.get_mut(split.line) {
                        line.v2 = split.original_v2;
                    }
                    if let Some(lid) = split.new_line.take() {
                        map.linedefs.remove(lid);
                    }
                    if let Some(sid) = split.new_right.take() {
                        map.sidedefs.remove(sid);
                    }
                    if let Some(sid) = split.new_left.take() {
                        map.sidedefs.remove(sid);
                    }
                    if let Some(vid) = split.new_v.take() {
                        map.vertices.remove(vid);
                    }
                }
                map.rebuild_sidedef_index();
            }
            Command::DeleteElements(state) => {
                state.current_v.clear();
                state.current_sec.clear();
                state.current_side.clear();
                state.current_line.clear();
                state.current_thing.clear();

                let mut remap_v: HashMap<VertexId, VertexId> = HashMap::new();
                for (old_id, v) in state.vertex_snaps.iter() {
                    let new_id = map.vertices.insert(*v);
                    remap_v.insert(*old_id, new_id);
                    state.current_v.push(new_id);
                }
                let mut remap_sec: HashMap<SectorId, SectorId> = HashMap::new();
                for (old_id, s) in state.sector_snaps.iter() {
                    let new_id = map.sectors.insert(s.clone());
                    remap_sec.insert(*old_id, new_id);
                    state.current_sec.push(new_id);
                }
                let mut remap_side: HashMap<SidedefId, SidedefId> = HashMap::new();
                for (old_id, s) in state.sidedef_snaps.iter() {
                    let mut new_side = s.clone();
                    if let Some(new_sec) = remap_sec.get(&new_side.sector) {
                        new_side.sector = *new_sec;
                    }
                    let new_id = map.sidedefs.insert(new_side);
                    remap_side.insert(*old_id, new_id);
                    state.current_side.push(new_id);
                }
                for (_old_id, l) in state.linedef_snaps.iter() {
                    let mut new_line = l.clone();
                    if let Some(nv) = remap_v.get(&new_line.v1) {
                        new_line.v1 = *nv;
                    }
                    if let Some(nv) = remap_v.get(&new_line.v2) {
                        new_line.v2 = *nv;
                    }
                    new_line.right = new_line.right.and_then(|s| remap_side.get(&s).copied());
                    new_line.left = new_line.left.and_then(|s| remap_side.get(&s).copied());
                    let new_id = map.linedefs.insert(new_line);
                    state.current_line.push(new_id);
                }
                for t in state.thing_snaps.iter() {
                    let new_id = map.things.insert(t.clone());
                    state.current_thing.push(new_id);
                }
                map.rebuild_sidedef_index();
            }
            Command::SetSectorIntField { id, field, old, .. } => {
                if let Some(s) = map.sectors.get_mut(*id) {
                    write_sector_int(s, *field, *old);
                }
            }
            Command::SetLinedefIntField { id, field, old, .. } => {
                if let Some(l) = map.linedefs.get_mut(*id) {
                    write_linedef_int(l, *field, *old);
                }
            }
            Command::SetThingIntField { id, field, old, .. } => {
                if let Some(t) = map.things.get_mut(*id) {
                    write_thing_int(t, *field, *old);
                }
            }
            Command::FlipLinedefs(ids) => {
                for id in ids.iter() {
                    if let Some(line) = map.linedefs.get_mut(*id) {
                        std::mem::swap(&mut line.v1, &mut line.v2);
                        std::mem::swap(&mut line.right, &mut line.left);
                    }
                }
            }
            Command::FlipSidedefs(ids) => {
                for id in ids.iter() {
                    if let Some(line) = map.linedefs.get_mut(*id) {
                        std::mem::swap(&mut line.right, &mut line.left);
                    }
                }
            }
            Command::SetSidedefOffsets(changes) => {
                // Revert: restore the previously-recorded old values for any
                // axis that was actually changed by the apply step.
                for c in changes.iter() {
                    if let Some(side) = map.sidedefs.get_mut(c.id) {
                        if c.new_x.is_some() {
                            side.x_offset = c.old_x;
                        }
                        if c.new_y.is_some() {
                            side.y_offset = c.old_y;
                        }
                    }
                }
            }
            Command::StitchLines(merges) => {
                // Walk in reverse so that later merges undo before earlier
                // ones in case they shared sidedefs/lines.
                for m in merges.iter_mut().rev() {
                    // 1. Re-insert absorbed.left sidedef if it existed.
                    let new_left_id = match &m.absorbed_left_snap {
                        Some((_orig_id, snap)) => Some(map.sidedefs.insert(snap.clone())),
                        None => None,
                    };
                    m.current_absorbed_left = new_left_id;
                    // 2. Detach the reassigned sidedef from keeper.left and
                    //    re-attach it to the absorbed line's right slot.
                    let stolen_right = m.keeper_new_left;
                    if let Some(line) = map.linedefs.get_mut(m.keeper) {
                        line.left = m.keeper_old_left;
                    }
                    // 3. Re-insert the absorbed linedef. Remap its left/right
                    //    fields: right gets the stolen sidedef back, left
                    //    gets the freshly re-inserted left sidedef.
                    let mut snap = m.absorbed_line_snap.clone();
                    snap.right = stolen_right;
                    snap.left = new_left_id;
                    let new_line_id = map.linedefs.insert(snap);
                    m.current_absorbed_line = Some(new_line_id);
                }
                map.rebuild_sidedef_index();
            }
            Command::PasteClipboard(state) => {
                // Remove in reverse order: things, linedefs, sidedefs,
                // sectors, vertices. Slotmap handles missing ids gracefully.
                for id in state.current_thing.drain(..) {
                    map.things.remove(id);
                }
                for id in state.current_line.drain(..) {
                    map.linedefs.remove(id);
                }
                for id in state.current_side.drain(..) {
                    map.sidedefs.remove(id);
                }
                for id in state.current_sec.drain(..) {
                    map.sectors.remove(id);
                }
                for id in state.current_v.drain(..) {
                    map.vertices.remove(id);
                }
                map.rebuild_sidedef_index();
            }
            Command::Batch(cmds) => {
                for cmd in cmds.iter_mut().rev() {
                    cmd.revert(map);
                }
            }
            Command::JoinSectors(state) => {
                // 1. Re-insert merged sectors and build orig->new remap.
                let mut sec_remap: HashMap<SectorId, SectorId> = HashMap::new();
                state.current_merged.clear();
                for (orig, snap) in &state.merged_snapshots {
                    let mut snap = snap.clone();
                    snap.sidedefs.clear(); // rebuilt below
                    let new_id = map.sectors.insert(snap);
                    sec_remap.insert(*orig, new_id);
                    state.current_merged.push(new_id);
                }
                // 2. Re-insert removed sidedefs (remapping their .sector).
                let mut side_remap: HashMap<SidedefId, SidedefId> = HashMap::new();
                state.current_removed_sides.clear();
                for (orig, snap) in &state.removed_sides {
                    let mut s = snap.clone();
                    if let Some(&new_sec) = sec_remap.get(&s.sector) {
                        s.sector = new_sec;
                    }
                    let new_id = map.sidedefs.insert(s);
                    side_remap.insert(*orig, new_id);
                    state.current_removed_sides.push(new_id);
                }
                // 3. Re-insert removed linedefs (remapping their right/left).
                state.current_removed_lines.clear();
                for (_orig, snap) in &state.removed_lines {
                    let mut l = snap.clone();
                    if let Some(s) = l.right {
                        if let Some(&new_s) = side_remap.get(&s) {
                            l.right = Some(new_s);
                        }
                    }
                    if let Some(s) = l.left {
                        if let Some(&new_s) = side_remap.get(&s) {
                            l.left = Some(new_s);
                        }
                    }
                    let new_id = map.linedefs.insert(l);
                    state.current_removed_lines.push(new_id);
                }
                // 4. Restore sidedef_changes: each affected sidedef's .sector
                //    goes back to its (possibly remapped) original.
                for (sid, old) in &state.sidedef_changes {
                    if let Some(side) = map.sidedefs.get_mut(*sid) {
                        let restore = sec_remap.get(old).copied().unwrap_or(*old);
                        side.sector = restore;
                    }
                }
                map.rebuild_sidedef_index();
            }
        }
    }
}

fn write_sector_int(s: &mut crate::map::MapSector, field: SectorIntField, value: i32) {
    match field {
        SectorIntField::FloorHeight => s.floor_height = value.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        SectorIntField::CeilingHeight => {
            s.ceiling_height = value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        }
        SectorIntField::Light => s.light = value.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        SectorIntField::Tag => s.tag = value.clamp(0, u16::MAX as i32) as u16,
        SectorIntField::Special => s.special = value.clamp(0, u16::MAX as i32) as u16,
    }
}

fn write_linedef_int(l: &mut crate::map::MapLinedef, field: LinedefIntField, value: i32) {
    match field {
        LinedefIntField::Flags => l.flags = value.clamp(0, u16::MAX as i32) as u16,
        LinedefIntField::Tag => l.tag = value.clamp(0, u16::MAX as i32) as u16,
        LinedefIntField::Arg0 => l.args[0] = value.clamp(0, 255) as u8,
        LinedefIntField::Arg1 => l.args[1] = value.clamp(0, 255) as u8,
        LinedefIntField::Arg2 => l.args[2] = value.clamp(0, 255) as u8,
        LinedefIntField::Arg3 => l.args[3] = value.clamp(0, 255) as u8,
        LinedefIntField::Arg4 => l.args[4] = value.clamp(0, 255) as u8,
    }
}

fn write_thing_int(t: &mut crate::map::MapThing, field: ThingIntField, value: i32) {
    match field {
        ThingIntField::Angle => t.angle = ((value % 360 + 360) % 360).clamp(0, u16::MAX as i32) as u16,
        ThingIntField::Flags => t.flags = value.clamp(0, u16::MAX as i32) as u16,
    }
}

fn write_sidedef_slot(side: &mut crate::map::MapSidedef, slot: SidedefSlot, value: TextureName) {
    match slot {
        SidedefSlot::Upper => side.upper_texture = value,
        SidedefSlot::Middle => side.middle_texture = value,
        SidedefSlot::Lower => side.lower_texture = value,
    }
}

fn write_sector_slot(sec: &mut crate::map::MapSector, slot: SectorSlot, value: TextureName) {
    match slot {
        SectorSlot::Floor => sec.floor_texture = value,
        SectorSlot::Ceiling => sec.ceiling_texture = value,
    }
}

#[derive(Debug, Default)]
pub struct UndoStack {
    past: Vec<Command>,
    future: Vec<Command>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a command. Clears the redo branch.
    pub fn push(&mut self, cmd: Command) {
        self.past.push(cmd);
        self.future.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn undo(&mut self, map: &mut Map) -> bool {
        match self.past.pop() {
            Some(mut cmd) => {
                cmd.revert(map);
                self.future.push(cmd);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, map: &mut Map) -> bool {
        match self.future.pop() {
            Some(mut cmd) => {
                cmd.apply(map);
                self.past.push(cmd);
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::MapFormat;
    use crate::map::MapVertex;

    fn map_with_vertex() -> (Map, VertexId) {
        let mut m = Map::new("T", MapFormat::Doom);
        let id = m.vertices.insert(MapVertex { x: 10, y: 20 });
        (m, id)
    }

    #[test]
    fn apply_then_revert_round_trips() {
        let (mut map, id) = map_with_vertex();
        let mut cmd = Command::MoveVertices(vec![VertexMove { id, dx: 5, dy: -3 }]);
        cmd.apply(&mut map);
        assert_eq!(map.vertices[id].x, 15);
        assert_eq!(map.vertices[id].y, 17);
        cmd.revert(&mut map);
        assert_eq!(map.vertices[id].x, 10);
        assert_eq!(map.vertices[id].y, 20);
    }

    #[test]
    fn undo_redo_chain() {
        let (mut map, id) = map_with_vertex();
        let mut stack = UndoStack::new();
        let mut cmd = Command::MoveVertices(vec![VertexMove { id, dx: 5, dy: 0 }]);
        cmd.apply(&mut map);
        stack.push(cmd);
        assert_eq!(map.vertices[id].x, 15);

        assert!(stack.undo(&mut map));
        assert_eq!(map.vertices[id].x, 10);
        assert!(!stack.can_undo());
        assert!(stack.can_redo());

        assert!(stack.redo(&mut map));
        assert_eq!(map.vertices[id].x, 15);
        assert!(!stack.can_redo());
    }

    #[test]
    fn merge_two_vertices_redirects_lines_and_collapses_degenerate() {
        use crate::map::{MapLinedef, MapVertex};
        let mut map = Map::new("T", MapFormat::Doom);
        // Three vertices, two lines: (v0-v1), (v1-v2). Merge v0 and v1.
        // Line (v0-v1) becomes (survivor=v0, survivor=v0) → degenerate, removed.
        // Line (v1-v2) becomes (survivor=v0, v2) → kept, v1 redirected.
        let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
        let v1 = map.vertices.insert(MapVertex { x: 16, y: 0 });
        let v2 = map.vertices.insert(MapVertex { x: 32, y: 0 });
        let l0 = map.linedefs.insert(MapLinedef {
            v1: v0,
            v2: v1,
            flags: 0,
            special: 0,
            args: [0; 5],
            tag: 0,
            right: None,
            left: None,
            fields: Default::default(),
        });
        let l1 = map.linedefs.insert(MapLinedef {
            v1: v1,
            v2: v2,
            flags: 0,
            special: 0,
            args: [0; 5],
            tag: 0,
            right: None,
            left: None,
            fields: Default::default(),
        });

        let state = compute_vertex_merge(&map, &[v0, v1]).expect("state");
        assert_eq!(state.removed_vertex_data.len(), 1, "v1 marked removed");
        assert_eq!(state.removed_line_data.len(), 1, "l0 degenerate");
        assert_eq!(state.redirected.len(), 1, "l1 redirected");

        let mut cmd = Command::MergeVertices(Box::new(state));
        cmd.apply(&mut map);

        assert_eq!(map.vertices.len(), 2, "v1 removed");
        assert_eq!(map.linedefs.len(), 1, "l0 removed");
        assert!(map.linedefs.get(l0).is_none());
        // Surviving line points at survivor + v2 with survivor at midpoint of v0/v1.
        let l1_now = &map.linedefs[l1];
        assert_eq!(l1_now.v1, v0, "v1 redirected to survivor v0");
        assert_eq!(l1_now.v2, v2);
        assert_eq!(map.vertices[v0].x, 8, "survivor moved to centroid x");

        // Undo restores everything.
        cmd.revert(&mut map);
        assert_eq!(map.vertices.len(), 3);
        assert_eq!(map.linedefs.len(), 2);
        assert_eq!(map.vertices[v0].x, 0, "survivor pos restored");
        let restored_l1 = &map.linedefs[l1];
        // l1.v1 should be remapped to whatever new id v1 got.
        assert!(map.vertices.contains_key(restored_l1.v1));
        assert_ne!(restored_l1.v1, v0, "redirected endpoint reverted");
    }

    #[test]
    fn split_linedef_inserts_midpoint_and_clones_sidedefs() {
        use crate::map::{MapLinedef, MapSidedef, MapVertex, TextureName};
        let mut map = Map::new("T", MapFormat::Doom);
        let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
        let v1 = map.vertices.insert(MapVertex { x: 64, y: 0 });
        let sec = map.sectors.insert(crate::map::MapSector {
            floor_height: 0,
            ceiling_height: 128,
            floor_texture: TextureName([0; 8]),
            ceiling_texture: TextureName([0; 8]),
            light: 192,
            special: 0,
            tag: 0,
            sidedefs: Vec::new(),
            fields: Default::default(),
        });
        let s0 = map.sidedefs.insert(MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"WALL\0\0\0\0"),
        });
        let line = map.linedefs.insert(MapLinedef {
            v1: v0,
            v2: v1,
            flags: 1,
            special: 26,
            args: [0; 5],
            tag: 7,
            right: Some(s0),
            left: None,
            fields: Default::default(),
        });

        let state = compute_split_lines(&map, &[line]).expect("state");
        let mut cmd = Command::SplitLinedefs(Box::new(state));
        cmd.apply(&mut map);

        assert_eq!(map.vertices.len(), 3, "midpoint inserted");
        assert_eq!(map.linedefs.len(), 2, "two halves");
        assert_eq!(map.sidedefs.len(), 2, "right sidedef cloned");

        // Both halves carry the original line's flags/special/tag.
        for (_, l) in &map.linedefs {
            assert_eq!(l.flags, 1);
            assert_eq!(l.special, 26);
            assert_eq!(l.tag, 7);
            assert!(l.right.is_some(), "right preserved on both halves");
            assert!(l.left.is_none());
        }

        cmd.revert(&mut map);
        assert_eq!(map.vertices.len(), 2, "midpoint removed on undo");
        assert_eq!(map.linedefs.len(), 1, "back to one line");
        assert_eq!(map.sidedefs.len(), 1, "cloned sidedef removed");
        let (_, l) = map.linedefs.iter().next().unwrap();
        assert_eq!(l.v2, v1, "v2 restored to original");
    }

    #[test]
    fn make_sector_assigns_sidedefs_inward() {
        use crate::map::{MapLinedef, MapVertex};
        let mut map = Map::new("T", MapFormat::Doom);
        // CCW square: (0,0) (64,0) (64,64) (0,64).
        let v = [
            map.vertices.insert(MapVertex { x: 0, y: 0 }),
            map.vertices.insert(MapVertex { x: 64, y: 0 }),
            map.vertices.insert(MapVertex { x: 64, y: 64 }),
            map.vertices.insert(MapVertex { x: 0, y: 64 }),
        ];
        let mk = |a, b| MapLinedef {
            v1: a,
            v2: b,
            flags: 0,
            special: 0,
            args: [0; 5],
            tag: 0,
            right: None,
            left: None,
            fields: Default::default(),
        };
        let lines = vec![
            map.linedefs.insert(mk(v[0], v[1])),
            map.linedefs.insert(mk(v[1], v[2])),
            map.linedefs.insert(mk(v[2], v[3])),
            map.linedefs.insert(mk(v[3], v[0])),
        ];

        let state = compute_make_sector(&map, &lines).expect("loop is closed");
        assert_eq!(state.line_assignments.len(), 4);

        let mut cmd = Command::MakeSector(Box::new(state));
        cmd.apply(&mut map);

        assert_eq!(map.sectors.len(), 1);
        assert_eq!(map.sidedefs.len(), 4);
        // For a CCW square walked v0->v1->v2->v3, the centroid is to the LEFT of
        // the walk direction, so each linedef's left side is inside.
        // walk v0->v1 matches linedef.v1->v2, so inside == left == linedef.left.
        let l01 = &map.linedefs[lines[0]];
        assert!(l01.left.is_some(), "v0->v1 left should be set");
        assert!(l01.right.is_none(), "v0->v1 right should remain unset");

        // Undo: linedefs revert to having no sides.
        cmd.revert(&mut map);
        assert_eq!(map.sectors.len(), 0);
        assert_eq!(map.sidedefs.len(), 0);
        for lid in &lines {
            let l = &map.linedefs[*lid];
            assert!(l.right.is_none() && l.left.is_none());
        }
    }

    #[test]
    fn delete_vertex_cascades_to_linedef_and_sidedefs() {
        use crate::map::{MapLinedef, MapSidedef, MapVertex, TextureName};
        let mut map = Map::new("T", MapFormat::Doom);
        let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
        let v1 = map.vertices.insert(MapVertex { x: 64, y: 0 });
        let sec = map.sectors.insert(crate::map::MapSector {
            floor_height: 0,
            ceiling_height: 128,
            floor_texture: TextureName([0; 8]),
            ceiling_texture: TextureName([0; 8]),
            light: 192,
            special: 0,
            tag: 0,
            sidedefs: Vec::new(),
            fields: Default::default(),
        });
        let s0 = map.sidedefs.insert(MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName([0; 8]),
        });
        let l0 = map.linedefs.insert(MapLinedef {
            v1: v0,
            v2: v1,
            flags: 0,
            special: 0,
            args: [0; 5],
            tag: 0,
            right: Some(s0),
            left: None,
            fields: Default::default(),
        });

        let mut sel_v = HashSet::new();
        sel_v.insert(v0);
        let state = collect_and_delete(&mut map, &sel_v, &HashSet::new(), &HashSet::new(), &HashSet::new());
        assert!(map.vertices.get(v0).is_none());
        assert!(map.linedefs.get(l0).is_none(), "linedef using deleted vertex must go");
        assert!(map.sidedefs.get(s0).is_none(), "sidedef of deleted linedef must go");
        assert_eq!(state.vertex_snaps.len(), 1);
        assert_eq!(state.linedef_snaps.len(), 1);
        assert_eq!(state.sidedef_snaps.len(), 1);
    }

    #[test]
    fn delete_element_undo_remaps_references() {
        use crate::map::{MapLinedef, MapSidedef, MapVertex, TextureName};
        let mut map = Map::new("T", MapFormat::Doom);
        let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
        let v1 = map.vertices.insert(MapVertex { x: 64, y: 0 });
        let sec = map.sectors.insert(crate::map::MapSector {
            floor_height: 0,
            ceiling_height: 128,
            floor_texture: TextureName([0; 8]),
            ceiling_texture: TextureName([0; 8]),
            light: 192,
            special: 0,
            tag: 0,
            sidedefs: Vec::new(),
            fields: Default::default(),
        });
        let s0 = map.sidedefs.insert(MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName([0; 8]),
        });
        map.linedefs.insert(MapLinedef {
            v1: v0,
            v2: v1,
            flags: 0,
            special: 0,
            args: [0; 5],
            tag: 0,
            right: Some(s0),
            left: None,
            fields: Default::default(),
        });

        let mut sel_v = HashSet::new();
        sel_v.insert(v0);
        let state = collect_and_delete(&mut map, &sel_v, &HashSet::new(), &HashSet::new(), &HashSet::new());
        let mut cmd = Command::DeleteElements(Box::new(state));
        assert_eq!(map.vertices.len(), 1);

        // Undo: must reinsert vertex/linedef/sidedef and patch refs.
        cmd.revert(&mut map);
        assert_eq!(map.vertices.len(), 2);
        assert_eq!(map.linedefs.len(), 1);
        assert_eq!(map.sidedefs.len(), 1);
        let (_, line) = map.linedefs.iter().next().unwrap();
        assert!(map.vertices.contains_key(line.v1));
        assert!(map.vertices.contains_key(line.v2));
        let right = line.right.unwrap();
        assert!(map.sidedefs.contains_key(right));
        let side = &map.sidedefs[right];
        assert!(map.sectors.contains_key(side.sector));
    }

    #[test]
    fn push_clears_redo_branch() {
        let (mut map, id) = map_with_vertex();
        let mut stack = UndoStack::new();
        let mut cmd_a = Command::MoveVertices(vec![VertexMove { id, dx: 5, dy: 0 }]);
        cmd_a.apply(&mut map);
        stack.push(cmd_a);
        stack.undo(&mut map);
        assert!(stack.can_redo());

        let mut cmd_b = Command::MoveVertices(vec![VertexMove { id, dx: 0, dy: 7 }]);
        cmd_b.apply(&mut map);
        stack.push(cmd_b);
        assert!(!stack.can_redo());
    }

    #[test]
    fn join_sectors_retargets_sidedefs_and_round_trips() {
        use crate::map::{MapSector, MapSidedef, TextureName};
        let mut map = Map::new("T", MapFormat::Doom);
        let sec_a = map.sectors.insert(MapSector {
            floor_height: 0,
            ceiling_height: 128,
            floor_texture: TextureName([0; 8]),
            ceiling_texture: TextureName([0; 8]),
            light: 160,
            special: 0,
            tag: 0,
            sidedefs: Vec::new(),
            fields: Default::default(),
        });
        let sec_b = map.sectors.insert(MapSector {
            floor_height: 16,
            ceiling_height: 144,
            floor_texture: TextureName([0; 8]),
            ceiling_texture: TextureName([0; 8]),
            light: 200,
            special: 0,
            tag: 0,
            sidedefs: Vec::new(),
            fields: Default::default(),
        });
        let side_a = map.sidedefs.insert(MapSidedef {
            sector: sec_a,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"A\0\0\0\0\0\0\0"),
        });
        let side_b = map.sidedefs.insert(MapSidedef {
            sector: sec_b,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"B\0\0\0\0\0\0\0"),
        });
        map.rebuild_sidedef_index();

        let state = compute_join_sectors(&map, &[sec_a, sec_b], false).expect("state");
        let mut cmd = Command::JoinSectors(Box::new(state));
        cmd.apply(&mut map);
        // sec_a survives, sec_b is gone, side_b retargets to sec_a.
        assert!(map.sectors.get(sec_a).is_some());
        assert!(map.sectors.get(sec_b).is_none());
        assert_eq!(map.sidedefs[side_a].sector, sec_a);
        assert_eq!(map.sidedefs[side_b].sector, sec_a);

        cmd.revert(&mut map);
        // sec_b is back (under a new id); side_b once again points at it.
        assert_eq!(map.sectors.len(), 2);
        // side_a unchanged.
        assert_eq!(map.sidedefs[side_a].sector, sec_a);
        // side_b's sector pointer should be a sector that is NOT sec_a.
        assert_ne!(map.sidedefs[side_b].sector, sec_a);
    }

    #[test]
    fn flip_linedef_swaps_endpoints_and_sides_and_round_trips() {
        use crate::map::{MapLinedef, MapSidedef, MapVertex, TextureName};
        let mut map = Map::new("T", MapFormat::Doom);
        let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
        let v1 = map.vertices.insert(MapVertex { x: 100, y: 0 });
        let sec = map.sectors.insert(crate::map::MapSector {
            floor_height: 0,
            ceiling_height: 128,
            floor_texture: TextureName([0; 8]),
            ceiling_texture: TextureName([0; 8]),
            light: 160,
            special: 0,
            tag: 0,
            sidedefs: Vec::new(),
            fields: Default::default(),
        });
        let s_right = map.sidedefs.insert(MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"R\0\0\0\0\0\0\0"),
        });
        let s_left = map.sidedefs.insert(MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"L\0\0\0\0\0\0\0"),
        });
        let line = map.linedefs.insert(MapLinedef {
            v1: v0,
            v2: v1,
            flags: 0,
            special: 0,
            args: [0; 5],
            tag: 0,
            right: Some(s_right),
            left: Some(s_left),
            fields: Default::default(),
        });

        let mut cmd = Command::FlipLinedefs(vec![line]);
        cmd.apply(&mut map);
        let l = map.linedefs.get(line).unwrap();
        assert_eq!(l.v1, v1);
        assert_eq!(l.v2, v0);
        assert_eq!(l.right, Some(s_left));
        assert_eq!(l.left, Some(s_right));

        cmd.revert(&mut map);
        let l = map.linedefs.get(line).unwrap();
        assert_eq!(l.v1, v0);
        assert_eq!(l.v2, v1);
        assert_eq!(l.right, Some(s_right));
        assert_eq!(l.left, Some(s_left));
    }
}
