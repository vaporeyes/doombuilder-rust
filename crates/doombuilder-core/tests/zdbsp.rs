// ABOUTME: End-to-end check that the zdbsp subprocess builder produces a
// ABOUTME: complete set of BSP lumps. Skips when zdbsp isn't on PATH.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use doombuilder_core::map::{
    save_map_as_pwad_with, MapLinedef, MapSector, MapSidedef, MapVertex, NodeBuilder, TextureName,
};
use doombuilder_core::wad::Wad;
use doombuilder_core::{Map, MapFormat};

fn zdbsp_on_path() -> Option<PathBuf> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let out = Command::new(cmd).arg("zdbsp").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let line = s.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(PathBuf::from(line))
    }
}

fn square_doom_map() -> Map {
    let mut map = Map::new("MAP01", MapFormat::Doom);
    let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
    let v1 = map.vertices.insert(MapVertex { x: 256, y: 0 });
    let v2 = map.vertices.insert(MapVertex { x: 256, y: 256 });
    let v3 = map.vertices.insert(MapVertex { x: 0, y: 256 });
    let sec = map.sectors.insert(MapSector {
        floor_height: 0,
        ceiling_height: 128,
        floor_texture: TextureName(*b"FLOOR5_4"),
        ceiling_texture: TextureName(*b"CEIL3_5\0"),
        light: 192,
        special: 0,
        tag: 0,
        sidedefs: Vec::new(),
        fields: Default::default(),
    });
    let mk_side = |map: &mut Map| {
        map.sidedefs.insert(MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"STARTAN3"),
        })
    };
    let s0 = mk_side(&mut map);
    let s1 = mk_side(&mut map);
    let s2 = mk_side(&mut map);
    let s3 = mk_side(&mut map);
    let mk_line = |a, b, side| MapLinedef {
        v1: a,
        v2: b,
        flags: 1,
        special: 0,
        args: [0; 5],
        tag: 0,
        right: Some(side),
        left: None,
        fields: Default::default(),
    };
    map.linedefs.insert(mk_line(v0, v1, s0));
    map.linedefs.insert(mk_line(v1, v2, s1));
    map.linedefs.insert(mk_line(v2, v3, s2));
    map.linedefs.insert(mk_line(v3, v0, s3));
    map.rebuild_sidedef_index();
    map
}

#[test]
fn zdbsp_produces_full_lump_set() {
    let Some(exe) = zdbsp_on_path() else {
        eprintln!("zdbsp not on PATH; skipping");
        return;
    };

    let map = square_doom_map();
    let builder = NodeBuilder::Zdbsp {
        exe,
        extra_args: Vec::<OsString>::new(),
    };
    let bytes = save_map_as_pwad_with(&map, &builder).expect("zdbsp save ok");
    let wad = Wad::from_bytes(bytes).expect("wad parses");

    // Every BSP-derived lump must be present and non-empty after zdbsp runs.
    for name in [
        "SEGS", "SSECTORS", "NODES", "BLOCKMAP", "REJECT", "VERTEXES",
    ] {
        let entry = wad.find(name).unwrap_or_else(|| panic!("missing {name}"));
        let bytes = wad.lump_bytes(entry).unwrap();
        assert!(!bytes.is_empty(), "{name} unexpectedly empty");
    }
}
