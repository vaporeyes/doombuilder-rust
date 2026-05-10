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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidedefSide {
    Right,
    Left,
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
        });
    }
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
                    let mid = MapVertex {
                        x: (v1.x + v2.x) / 2,
                        y: (v1.y + v2.y) / 2,
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
}
