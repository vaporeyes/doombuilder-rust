// ABOUTME: Load a named map out of a WAD into the Map slotmap representation.
// ABOUTME: Resolves on-disk u16 indices into stable generational IDs as we go.

use crate::error::{Error, Result};
use crate::format::MapFormat;
use crate::wad::lumps::{
    LinedefDoom, LinedefHexen, Sector as RawSector, Sidedef as RawSidedef, ThingDoom, ThingHexen,
    Vertex as RawVertex,
};
use crate::wad::{DirEntry, Wad};

use super::{
    Map, MapLinedef, MapSector, MapSidedef, MapThing, MapVertex, SectorId, SidedefId, TextureName,
    VertexId,
};

const MAX_MAP_LUMPS: usize = 16;

/// Lump names that legitimately appear inside a map block. Used to delimit a map
/// when iterating the WAD directory after the map marker.
const MAP_LUMP_NAMES: &[&str] = &[
    "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT",
    "BLOCKMAP", "BEHAVIOR", "SCRIPTS", "TEXTMAP", "ZNODES", "DIALOGUE", "ENDMAP",
];

/// Convenience newtype wrapping a map name (e.g. "MAP01", "E1M1").
#[derive(Debug, Clone)]
pub struct MapName(pub String);

impl<S: Into<String>> From<S> for MapName {
    fn from(s: S) -> Self {
        MapName(s.into())
    }
}

pub fn load_doom(wad: &Wad, name: impl Into<MapName>) -> Result<Map> {
    load(wad, name.into(), MapFormat::Doom)
}

pub fn load_hexen(wad: &Wad, name: impl Into<MapName>) -> Result<Map> {
    load(wad, name.into(), MapFormat::Hexen)
}

/// Detect Doom vs Hexen format by inspecting the map's lump block.
/// A BEHAVIOR lump after the map marker means Hexen; otherwise Doom.
pub fn detect_format(wad: &Wad, name: &str) -> Result<MapFormat> {
    let lumps = locate_map(wad, name)?;
    let is_hexen = lumps
        .iter()
        .any(|e| e.name_str().eq_ignore_ascii_case("BEHAVIOR"));
    Ok(if is_hexen {
        MapFormat::Hexen
    } else {
        MapFormat::Doom
    })
}

/// Convenience: detect format then load.
pub fn load_auto(wad: &Wad, name: &str) -> Result<Map> {
    let format = detect_format(wad, name)?;
    load(wad, MapName(name.to_string()), format)
}

fn load(wad: &Wad, name: MapName, format: MapFormat) -> Result<Map> {
    let lumps = locate_map(wad, &name.0)?;

    let vertex_lump = require_lump(&lumps, "VERTEXES", &name.0)?;
    let sector_lump = require_lump(&lumps, "SECTORS", &name.0)?;
    let sidedef_lump = require_lump(&lumps, "SIDEDEFS", &name.0)?;
    let linedef_lump = require_lump(&lumps, "LINEDEFS", &name.0)?;
    let thing_lump = require_lump(&lumps, "THINGS", &name.0)?;

    let raw_vertices: &[RawVertex] = wad.lump_as_slice(vertex_lump)?;
    let raw_sectors: &[RawSector] = wad.lump_as_slice(sector_lump)?;
    let raw_sidedefs: &[RawSidedef] = wad.lump_as_slice(sidedef_lump)?;

    let mut map = Map::new(&name.0, format);

    let vertex_ids: Vec<VertexId> = raw_vertices
        .iter()
        .map(|v| {
            map.vertices.insert(MapVertex {
                x: v.x as i32,
                y: v.y as i32,
            })
        })
        .collect();

    let sector_ids: Vec<SectorId> = raw_sectors
        .iter()
        .map(|s| {
            map.sectors.insert(MapSector {
                floor_height: s.floor_height,
                ceiling_height: s.ceiling_height,
                floor_texture: TextureName(s.floor_texture),
                ceiling_texture: TextureName(s.ceiling_texture),
                light: s.light,
                special: s.special,
                tag: s.tag,
                sidedefs: Vec::new(),
                fields: Default::default(),
            })
        })
        .collect();

    let sidedef_ids: Vec<SidedefId> = raw_sidedefs
        .iter()
        .map(|s| -> Result<SidedefId> {
            let sector = resolve_index(&sector_ids, s.sector, "sidedef.sector")?;
            Ok(map.sidedefs.insert(MapSidedef {
                sector,
                x_offset: s.x_offset,
                y_offset: s.y_offset,
                upper_texture: TextureName(s.upper_texture),
                lower_texture: TextureName(s.lower_texture),
                middle_texture: TextureName(s.middle_texture),
            }))
        })
        .collect::<Result<_>>()?;

    match format {
        MapFormat::Doom => load_linedefs_doom(wad, linedef_lump, &mut map, &vertex_ids, &sidedef_ids)?,
        MapFormat::Hexen => load_linedefs_hexen(wad, linedef_lump, &mut map, &vertex_ids, &sidedef_ids)?,
    }

    match format {
        MapFormat::Doom => load_things_doom(wad, thing_lump, &mut map)?,
        MapFormat::Hexen => load_things_hexen(wad, thing_lump, &mut map)?,
    }

    map.rebuild_sidedef_index();
    Ok(map)
}

fn load_linedefs_doom(
    wad: &Wad,
    lump: &DirEntry,
    map: &mut Map,
    vertex_ids: &[VertexId],
    sidedef_ids: &[SidedefId],
) -> Result<()> {
    let raw: &[LinedefDoom] = wad.lump_as_slice(lump)?;
    for line in raw {
        let v1 = resolve_index(vertex_ids, line.start_vertex, "linedef.start_vertex")?;
        let v2 = resolve_index(vertex_ids, line.end_vertex, "linedef.end_vertex")?;
        let right = resolve_optional_sidedef(sidedef_ids, line.right_sidedef)?;
        let left = resolve_optional_sidedef(sidedef_ids, line.left_sidedef)?;
        map.linedefs.insert(MapLinedef {
            v1,
            v2,
            flags: line.flags,
            special: line.special,
            args: [0; 5],
            tag: line.tag,
            right,
            left,
            fields: Default::default(),
        });
    }
    Ok(())
}

fn load_linedefs_hexen(
    wad: &Wad,
    lump: &DirEntry,
    map: &mut Map,
    vertex_ids: &[VertexId],
    sidedef_ids: &[SidedefId],
) -> Result<()> {
    let raw: &[LinedefHexen] = wad.lump_as_slice(lump)?;
    for line in raw {
        let v1 = resolve_index(vertex_ids, line.start_vertex, "linedef.start_vertex")?;
        let v2 = resolve_index(vertex_ids, line.end_vertex, "linedef.end_vertex")?;
        let right = resolve_optional_sidedef(sidedef_ids, line.right_sidedef)?;
        let left = resolve_optional_sidedef(sidedef_ids, line.left_sidedef)?;
        map.linedefs.insert(MapLinedef {
            v1,
            v2,
            flags: line.flags,
            special: line.special as u16,
            args: line.args,
            tag: 0,
            right,
            left,
            fields: Default::default(),
        });
    }
    Ok(())
}

fn load_things_doom(wad: &Wad, lump: &DirEntry, map: &mut Map) -> Result<()> {
    let raw: &[ThingDoom] = wad.lump_as_slice(lump)?;
    for t in raw {
        map.things.insert(MapThing {
            x: t.x as i32,
            y: t.y as i32,
            angle: t.angle,
            kind: t.kind,
            flags: t.flags,
            tid: 0,
            z: 0,
            special: 0,
            args: [0; 5],
        });
    }
    Ok(())
}

fn load_things_hexen(wad: &Wad, lump: &DirEntry, map: &mut Map) -> Result<()> {
    let raw: &[ThingHexen] = wad.lump_as_slice(lump)?;
    for t in raw {
        map.things.insert(MapThing {
            x: t.x as i32,
            y: t.y as i32,
            angle: t.angle,
            kind: t.kind,
            flags: t.flags,
            tid: t.tid,
            z: t.z,
            special: t.special,
            args: t.args,
        });
    }
    Ok(())
}

fn resolve_index<T: slotmap::Key>(table: &[T], idx: u16, what: &str) -> Result<T> {
    table
        .get(idx as usize)
        .copied()
        .ok_or_else(|| Error::BadReference {
            what: what.to_string(),
            index: idx as usize,
            len: table.len(),
        })
}

fn resolve_optional_sidedef(table: &[SidedefId], idx: u16) -> Result<Option<SidedefId>> {
    if idx == 0xFFFF {
        Ok(None)
    } else {
        table
            .get(idx as usize)
            .copied()
            .map(Some)
            .ok_or_else(|| Error::BadReference {
                what: "linedef.sidedef".to_string(),
                index: idx as usize,
                len: table.len(),
            })
    }
}

fn locate_map<'a>(wad: &'a Wad, name: &str) -> Result<Vec<&'a DirEntry>> {
    let dir = wad.directory();
    let marker_idx = dir
        .iter()
        .position(|e| e.name_str().eq_ignore_ascii_case(name))
        .ok_or_else(|| Error::MapNotFound(name.to_string()))?;

    let mut lumps = Vec::with_capacity(MAX_MAP_LUMPS);
    for entry in dir
        .iter()
        .skip(marker_idx + 1)
        .take(MAX_MAP_LUMPS)
    {
        let n = entry.name_str();
        if MAP_LUMP_NAMES.iter().any(|w| w.eq_ignore_ascii_case(n)) {
            lumps.push(entry);
        } else {
            break;
        }
    }
    Ok(lumps)
}

fn require_lump<'a>(
    lumps: &'a [&'a DirEntry],
    name: &str,
    map_name: &str,
) -> Result<&'a DirEntry> {
    lumps
        .iter()
        .copied()
        .find(|e| e.name_str().eq_ignore_ascii_case(name))
        .ok_or_else(|| Error::MissingMapLump {
            map: map_name.to_string(),
            lump: name.to_string(),
        })
}
