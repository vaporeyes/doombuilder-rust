// ABOUTME: Mutable, indexed map representation. Geometry is stored in slotmaps
// ABOUTME: keyed by generational IDs; cross-references are by ID, never by &/&mut.

pub mod ids;
mod load;

pub use ids::{LinedefId, SectorId, SidedefId, ThingId, VertexId};
pub use load::{detect_format, load_auto, load_doom, load_hexen, MapName};

use slotmap::SlotMap;

use crate::format::MapFormat;

#[derive(Debug, Clone, Copy)]
pub struct MapVertex {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone)]
pub struct MapLinedef {
    pub v1: VertexId,
    pub v2: VertexId,
    pub flags: u16,
    /// Doom: 16-bit special. Hexen: 8-bit special promoted to 16-bit so callers
    /// can read it uniformly; `args` is the Hexen action arg block (zero on Doom).
    pub special: u16,
    pub args: [u8; 5],
    /// Doom-only sector tag. Hexen carries equivalent info in `args`.
    pub tag: u16,
    pub right: Option<SidedefId>,
    pub left: Option<SidedefId>,
}

#[derive(Debug, Clone)]
pub struct MapSidedef {
    pub sector: SectorId,
    pub x_offset: i16,
    pub y_offset: i16,
    pub upper_texture: TextureName,
    pub lower_texture: TextureName,
    pub middle_texture: TextureName,
}

#[derive(Debug, Clone)]
pub struct MapSector {
    pub floor_height: i16,
    pub ceiling_height: i16,
    pub floor_texture: TextureName,
    pub ceiling_texture: TextureName,
    pub light: i16,
    pub special: u16,
    pub tag: u16,
    /// Derived index of sidedefs whose `sector` field points here.
    /// Rebuilt by `Map::rebuild_sidedef_index`; keep in sync on mutations.
    pub sidedefs: Vec<SidedefId>,
}

#[derive(Debug, Clone)]
pub struct MapThing {
    pub x: i32,
    pub y: i32,
    pub angle: u16,
    pub kind: u16,
    pub flags: u16,
    /// Hexen-only fields. Zero on Doom-format maps.
    pub tid: u16,
    pub z: i16,
    pub special: u8,
    pub args: [u8; 5],
}

/// 8-byte uppercase ASCII texture/flat name, trimmed at the first NUL.
/// Wrapped to keep size cheap (`Copy`) and avoid heap allocations in lumps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureName(pub [u8; 8]);

impl TextureName {
    pub fn as_str(&self) -> &str {
        let end = self.0.iter().position(|&b| b == 0).unwrap_or(8);
        std::str::from_utf8(&self.0[..end]).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.0[0] == 0 || &self.0[..1] == b"-"
    }
}

#[derive(Debug, Clone)]
pub struct Map {
    pub name: String,
    pub format: MapFormat,
    pub vertices: SlotMap<VertexId, MapVertex>,
    pub linedefs: SlotMap<LinedefId, MapLinedef>,
    pub sidedefs: SlotMap<SidedefId, MapSidedef>,
    pub sectors: SlotMap<SectorId, MapSector>,
    pub things: SlotMap<ThingId, MapThing>,
}

impl Map {
    pub fn new(name: impl Into<String>, format: MapFormat) -> Self {
        Self {
            name: name.into(),
            format,
            vertices: SlotMap::with_key(),
            linedefs: SlotMap::with_key(),
            sidedefs: SlotMap::with_key(),
            sectors: SlotMap::with_key(),
            things: SlotMap::with_key(),
        }
    }

    /// Recompute every sector's `sidedefs` cache from the current sidedef table.
    /// O(S) where S = number of sidedefs.
    pub fn rebuild_sidedef_index(&mut self) {
        for (_, sector) in self.sectors.iter_mut() {
            sector.sidedefs.clear();
        }
        for (sid, side) in self.sidedefs.iter() {
            if let Some(sector) = self.sectors.get_mut(side.sector) {
                sector.sidedefs.push(sid);
            }
        }
    }
}
