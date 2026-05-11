// ABOUTME: Serialize a Map back to Doom or Hexen format lumps. BSP lumps
// ABOUTME: (SEGS / SSECTORS / NODES / REJECT / BLOCKMAP) are produced by
// ABOUTME: `map::nodes::build_nodes` so output is engine-playable.

use std::collections::HashMap;

use crate::error::Result;
use crate::format::MapFormat;
use crate::map::node_builder::{self, NodeBuilder};
use crate::map::{LinedefId, Map, SectorId, SidedefId, VertexId};
use crate::wad::LumpEntry;

/// Produce the ordered lump list for a single map, using the bundled node
/// builder. Panics never (the builtin builder is infallible); kept Result-free
/// so existing callers don't need updating.
pub fn serialize_map(map: &Map) -> Vec<LumpEntry<'_>> {
    // Builtin is infallible, so unwrap is safe.
    serialize_map_with(map, &NodeBuilder::Builtin).expect("builtin node builder is infallible")
}

/// Like `serialize_map` but takes a builder choice. Returns an error only
/// when an external builder (e.g. zdbsp) fails to run or produces bad output.
pub fn serialize_map_with<'a>(map: &'a Map, builder: &NodeBuilder) -> Result<Vec<LumpEntry<'a>>> {
    let vertex_idx: HashMap<VertexId, u16> = map
        .vertices
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i as u16))
        .collect();
    let sidedef_idx: HashMap<SidedefId, u16> = map
        .sidedefs
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i as u16))
        .collect();
    let sector_idx: HashMap<SectorId, u16> = map
        .sectors
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i as u16))
        .collect();

    let nodes = node_builder::build(map, builder)?;

    let things = serialize_things(map);
    let linedefs = serialize_linedefs(map, &vertex_idx, &sidedef_idx);
    let sidedefs = serialize_sidedefs(map, &sector_idx);
    let mut vertexes = serialize_vertexes(map);
    for (x, y) in &nodes.extra_vertices {
        vertexes.extend_from_slice(&x.to_le_bytes());
        vertexes.extend_from_slice(&y.to_le_bytes());
    }
    let sectors = serialize_sectors(map);

    let mut lumps: Vec<LumpEntry<'_>> = Vec::new();
    lumps.push(LumpEntry {
        name: &map.name,
        data: Vec::new(),
    });
    lumps.push(LumpEntry { name: "THINGS", data: things });
    lumps.push(LumpEntry { name: "LINEDEFS", data: linedefs });
    lumps.push(LumpEntry { name: "SIDEDEFS", data: sidedefs });
    lumps.push(LumpEntry { name: "VERTEXES", data: vertexes });
    lumps.push(LumpEntry { name: "SEGS", data: nodes.segs });
    lumps.push(LumpEntry { name: "SSECTORS", data: nodes.ssectors });
    lumps.push(LumpEntry { name: "NODES", data: nodes.nodes });
    lumps.push(LumpEntry { name: "SECTORS", data: sectors });
    lumps.push(LumpEntry { name: "REJECT", data: nodes.reject });
    lumps.push(LumpEntry { name: "BLOCKMAP", data: nodes.blockmap });
    if map.format == MapFormat::Hexen {
        lumps.push(LumpEntry { name: "BEHAVIOR", data: Vec::new() });
    }
    Ok(lumps)
}

/// Convenience wrapper: serialize then pack as PWAD bytes.
pub fn save_map_as_pwad(map: &Map) -> Vec<u8> {
    let lumps = serialize_map(map);
    crate::wad::write_pwad(&lumps)
}

/// Like `save_map_as_pwad` but lets the caller pick the node builder.
pub fn save_map_as_pwad_with(map: &Map, builder: &NodeBuilder) -> Result<Vec<u8>> {
    let lumps = serialize_map_with(map, builder)?;
    Ok(crate::wad::write_pwad(&lumps))
}

pub(crate) fn serialize_vertexes(map: &Map) -> Vec<u8> {
    let mut out = Vec::with_capacity(map.vertices.len() * 4);
    for (_, v) in &map.vertices {
        out.extend_from_slice(&(v.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes());
        out.extend_from_slice(&(v.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes());
    }
    out
}

pub(crate) fn serialize_sectors(map: &Map) -> Vec<u8> {
    let mut out = Vec::with_capacity(map.sectors.len() * 26);
    for (_, s) in &map.sectors {
        out.extend_from_slice(&s.floor_height.to_le_bytes());
        out.extend_from_slice(&s.ceiling_height.to_le_bytes());
        // Flats use the same "-" convention as wall textures for unset names.
        out.extend_from_slice(&normalize_tex_name(&s.floor_texture.0));
        out.extend_from_slice(&normalize_tex_name(&s.ceiling_texture.0));
        out.extend_from_slice(&s.light.to_le_bytes());
        out.extend_from_slice(&s.special.to_le_bytes());
        out.extend_from_slice(&s.tag.to_le_bytes());
    }
    out
}

pub(crate) fn serialize_sidedefs(map: &Map, sector_idx: &HashMap<SectorId, u16>) -> Vec<u8> {
    let mut out = Vec::with_capacity(map.sidedefs.len() * 30);
    for (_, s) in &map.sidedefs {
        out.extend_from_slice(&s.x_offset.to_le_bytes());
        out.extend_from_slice(&s.y_offset.to_le_bytes());
        // Doom convention: empty/all-NUL texture name is forbidden in the
        // lump; "-" (dash) means "no texture". gzdoom's LoadSideDefs2
        // null-derefs in the texture-resolution path when given all-NULs.
        out.extend_from_slice(&normalize_tex_name(&s.upper_texture.0));
        out.extend_from_slice(&normalize_tex_name(&s.lower_texture.0));
        out.extend_from_slice(&normalize_tex_name(&s.middle_texture.0));
        let sec = sector_idx.get(&s.sector).copied().unwrap_or(0);
        out.extend_from_slice(&sec.to_le_bytes());
    }
    out
}

/// Coerce a texture name into the on-disk form. Empty / all-NUL names become
/// `"-\0\0\0\0\0\0\0"` (the standard Doom "no texture" placeholder).
fn normalize_tex_name(bytes: &[u8; 8]) -> [u8; 8] {
    if bytes[0] == 0 {
        let mut out = [0u8; 8];
        out[0] = b'-';
        out
    } else {
        *bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{compute_make_sector, Command};
    use crate::format::MapFormat;
    use crate::map::{Map, MapLinedef, MapVertex};

    /// Repro for the gzdoom LoadSideDefs2 crash: drawing a rectangle and
    /// running Make Sector left upper/lower texture names all-NUL, which
    /// gzdoom's texture-resolution path null-derefs. The lump must use "-".
    #[test]
    fn make_sector_save_writes_dash_for_empty_textures() {
        let mut map = Map::new("MAP01", MapFormat::Doom);
        let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
        let v1 = map.vertices.insert(MapVertex { x: 128, y: 0 });
        let v2 = map.vertices.insert(MapVertex { x: 128, y: 128 });
        let v3 = map.vertices.insert(MapVertex { x: 0, y: 128 });
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
        let l0 = map.linedefs.insert(mk(v0, v1));
        let l1 = map.linedefs.insert(mk(v1, v2));
        let l2 = map.linedefs.insert(mk(v2, v3));
        let l3 = map.linedefs.insert(mk(v3, v0));

        let state = compute_make_sector(&map, &[l0, l1, l2, l3]).expect("make sector");
        let mut cmd = Command::MakeSector(Box::new(state));
        cmd.apply(&mut map);

        let lumps = serialize_map(&map);
        let sidedefs = &lumps.iter().find(|l| l.name == "SIDEDEFS").unwrap().data;
        assert_eq!(sidedefs.len(), 4 * 30, "4 sidedefs of 30 bytes each");
        // Each record's upper texture lives at offset 4..12. With the fix
        // applied, that should start with '-' (0x2D), not NUL.
        for i in 0..4 {
            let upper = &sidedefs[i * 30 + 4..i * 30 + 12];
            assert_eq!(
                upper[0], b'-',
                "sidedef {i}: upper texture must be '-' placeholder, got {:?}",
                upper
            );
        }
    }
}

pub(crate) fn serialize_linedefs(
    map: &Map,
    vertex_idx: &HashMap<VertexId, u16>,
    sidedef_idx: &HashMap<SidedefId, u16>,
) -> Vec<u8> {
    let stride = if map.format == MapFormat::Hexen { 16 } else { 14 };
    let mut out = Vec::with_capacity(map.linedefs.len() * stride);
    for (_, l) in &map.linedefs {
        let v1 = vertex_idx.get(&l.v1).copied().unwrap_or(0);
        let v2 = vertex_idx.get(&l.v2).copied().unwrap_or(0);
        let right = l
            .right
            .and_then(|s| sidedef_idx.get(&s).copied())
            .unwrap_or(0xFFFF);
        let left = l
            .left
            .and_then(|s| sidedef_idx.get(&s).copied())
            .unwrap_or(0xFFFF);
        out.extend_from_slice(&v1.to_le_bytes());
        out.extend_from_slice(&v2.to_le_bytes());
        out.extend_from_slice(&l.flags.to_le_bytes());
        match map.format {
            MapFormat::Doom => {
                out.extend_from_slice(&l.special.to_le_bytes());
                out.extend_from_slice(&l.tag.to_le_bytes());
            }
            MapFormat::Hexen => {
                out.push(l.special as u8);
                out.extend_from_slice(&l.args);
            }
        }
        out.extend_from_slice(&right.to_le_bytes());
        out.extend_from_slice(&left.to_le_bytes());
    }
    out
}

fn _suppress_unused(_id: LinedefId) {}

pub(crate) fn serialize_things(map: &Map) -> Vec<u8> {
    let stride = if map.format == MapFormat::Hexen { 20 } else { 10 };
    let mut out = Vec::with_capacity(map.things.len() * stride);
    for (_, t) in &map.things {
        match map.format {
            MapFormat::Doom => {
                out.extend_from_slice(
                    &(t.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes(),
                );
                out.extend_from_slice(
                    &(t.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes(),
                );
                out.extend_from_slice(&t.angle.to_le_bytes());
                out.extend_from_slice(&t.kind.to_le_bytes());
                out.extend_from_slice(&t.flags.to_le_bytes());
            }
            MapFormat::Hexen => {
                out.extend_from_slice(&t.tid.to_le_bytes());
                out.extend_from_slice(
                    &(t.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes(),
                );
                out.extend_from_slice(
                    &(t.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes(),
                );
                out.extend_from_slice(&t.z.to_le_bytes());
                out.extend_from_slice(&t.angle.to_le_bytes());
                out.extend_from_slice(&t.kind.to_le_bytes());
                out.extend_from_slice(&t.flags.to_le_bytes());
                out.push(t.special);
                out.extend_from_slice(&t.args);
            }
        }
    }
    out
}
