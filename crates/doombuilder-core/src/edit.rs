// ABOUTME: Editing commands and an undo/redo stack. Commands are diffs that
// ABOUTME: can be applied or reverted in O(k) where k is the number of elements
// ABOUTME: touched. Snapshot-of-Map is intentionally avoided to keep undo cheap.

use crate::map::{Map, SectorId, SidedefId, TextureName, VertexId};

#[derive(Debug, Clone)]
pub struct VertexMove {
    pub id: VertexId,
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
}

impl Command {
    pub fn apply(&self, map: &mut Map) {
        match self {
            Command::MoveVertices(moves) => {
                for m in moves {
                    if let Some(v) = map.vertices.get_mut(m.id) {
                        v.x = v.x.saturating_add(m.dx);
                        v.y = v.y.saturating_add(m.dy);
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
        }
    }

    pub fn revert(&self, map: &mut Map) {
        match self {
            Command::MoveVertices(moves) => {
                for m in moves {
                    if let Some(v) = map.vertices.get_mut(m.id) {
                        v.x = v.x.saturating_sub(m.dx);
                        v.y = v.y.saturating_sub(m.dy);
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
        }
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
            Some(cmd) => {
                cmd.revert(map);
                self.future.push(cmd);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, map: &mut Map) -> bool {
        match self.future.pop() {
            Some(cmd) => {
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
        let cmd = Command::MoveVertices(vec![VertexMove { id, dx: 5, dy: -3 }]);
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
        let cmd = Command::MoveVertices(vec![VertexMove { id, dx: 5, dy: 0 }]);
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
    fn push_clears_redo_branch() {
        let (mut map, id) = map_with_vertex();
        let mut stack = UndoStack::new();
        let cmd_a = Command::MoveVertices(vec![VertexMove { id, dx: 5, dy: 0 }]);
        cmd_a.apply(&mut map);
        stack.push(cmd_a);
        stack.undo(&mut map);
        assert!(stack.can_redo());

        let cmd_b = Command::MoveVertices(vec![VertexMove { id, dx: 0, dy: 7 }]);
        cmd_b.apply(&mut map);
        stack.push(cmd_b);
        assert!(!stack.can_redo());
    }
}
